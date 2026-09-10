mod bootstrap;
mod cage;
mod config_io;
mod connections;
mod control_helpers;
mod control_loop;
mod geomag;
mod history;
mod io_poll;
mod logging;
mod netcfg;
mod satellite_tracking;
mod satellite_ui;
mod setpoint;
mod ui;
mod ui_helpers;

use history::PlotHistory;
use satellite_ui::{SatSearchState, TrackedSat};

use eframe::egui::{self, Color32};
use igrf_core::geomagnetism::GeomagnetismResult;
use igrf_core::{
    AppConfig, CalculationService, CalibrationSettings, ContourSegment, DisplayMode,
    FilterSettings, MapGrid, PidController, PidSettings, ProcessedData, SensorService,
    SetpointProfile, SlewLimiter,
};
use igrf_io::{
    ControllerReplyCounter, CsvLogger, DriveInfo, MagsonSample, MagsonTcpClient, SerialPortManager,
    SetpointServer,
};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime};

const CONFIG_PATH: &str = "SystemConfig.json";
const CONTOUR_LINE_COLOR: Color32 = Color32::WHITE;
const UI_INTERVAL: Duration = Duration::from_millis(50);
const AXES: [char; 3] = ['X', 'Y', 'Z'];
const STOP_RED: Color32 = Color32::from_rgb(170, 45, 45);
/// Cycled by index so each tracked satellite gets a stable, distinct color
/// across the ground-track plot and the field-vs-time legend.
const SATELLITE_COLORS: [Color32; 6] = [
    Color32::RED,
    Color32::from_rgb(80, 200, 120),
    Color32::from_rgb(90, 150, 240),
    Color32::from_rgb(230, 170, 60),
    Color32::from_rgb(200, 90, 220),
    Color32::from_rgb(80, 220, 220),
];

fn main() -> eframe::Result {
    // Load `.env` from the working directory (or any parent) into the process
    // environment before anything reads it. Missing file is fine - real
    // environment variables still work, and so does an app that never fetches.
    let _ = dotenvy::dotenv();

    // The display mode and UI scale are read here, before the window exists,
    // because they decide how the window is created.
    let (config, config_problem) = AppConfig::load(CONFIG_PATH);

    let viewport = match config.display.mode {
        // Borderless: no window-manager title bar for a finger to mis-tap, and
        // `with_monitor` pins the fullscreen rect to the whole named screen.
        DisplayMode::Fullscreen => egui::ViewportBuilder::default()
            .with_fullscreen(true)
            .with_decorations(false)
            .with_monitor(config.display.fullscreen_monitor),
        // Fits the embedded 1024x600 panel by default. `with_min_inner_size` is
        // a floor, not a suggestion, so a small screen never gets a window it
        // cannot fit on either.
        DisplayMode::Window => egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 600.0])
            .with_min_inner_size([1024.0, 600.0]),
    };

    // `mut` is used only in the Linux-desktop block below.
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    // Run on X11 / XWayland rather than the native Wayland backend on Linux.
    // winit's Wayland fullscreen keeps a client-side title-bar strip across the
    // top of the screen on GNOME/Mutter; under X11 the window manager owns
    // fullscreen and it covers the whole panel. Set IGRF_FORCE_WAYLAND=1 to
    // opt back into the Wayland backend (e.g. on a HiDPI laptop where XWayland
    // looks blurry).
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("IGRF_FORCE_WAYLAND").is_none() {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11 as _;
            builder.with_x11();
        }));
    }

    eframe::run_native(
        "IGRF control",
        options,
        Box::new(move |cc| {
            // A finger on a touchscreen needs bigger touch targets; the scale
            // applies to fonts, buttons and spacing alike.
            cc.egui_ctx.set_zoom_factor(config.display.ui_scale);
            Ok(Box::new(IgrfApp::new(cc, config, config_problem)))
        }),
    )
}

/// Where the commanded field comes from. Only one is live at a time, so a
/// profile cannot fight a socket for the coils.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SetpointSource {
    /// Typed into the UI.
    Manual,
    /// Replayed from a CSV of `time_s,bx_nt,by_nt,bz_nt`.
    Profile,
    /// Pushed over UDP by an external propagator.
    Socket,
}

impl SetpointSource {
    fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Profile => "CSV profile",
            Self::Socket => "UDP socket",
        }
    }
}

/// Separated UI Tab IGRF Control and IGRF Model
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum AppTab {
    #[default]
    Control,
    Model,
    Settings,
}

impl AppTab {
    fn label(self) -> &'static str {
        match self {
            Self::Control => "IGRF Control",
            Self::Model => "IGRF Model",
            Self::Settings => "Settings",
        }
    }
}

/// One row in either file list of the Config / logging panel.
struct FileRow {
    name: String,
    is_dir: bool,
    size_bytes: u64,
}

struct IgrfApp {
    config: AppConfig,
    sensor_port: String,
    sensor_baud: u32,
    controller_port: String,
    controller_baud: u32,
    magson_ip: String,
    magson_port: u16,
    log_path: String,
    log_files: Vec<FileRow>,
    log_files_status: String,
    log_files_scanned: bool,
    log_files_sel: Option<String>,
    ext_drives: Vec<DriveInfo>,
    ext_drive_sel: usize,
    ext_scanned: bool,
    ext_cwd: Option<PathBuf>,
    ext_entries: Vec<FileRow>,
    ext_status: String,
    ext_sel: Option<String>,
    transfer_status: String,
    available_ports: Vec<String>,
    lan_profiles: Vec<netcfg::LanProfile>,
    lan_selected: usize,
    lan_cidr: String,
    lan_task: Option<Receiver<Result<String, String>>>,

    sensor_manager: SerialPortManager,
    controller_manager: SerialPortManager,
    controller_replies: ControllerReplyCounter,
    controller_sent: u64,
    controller_rejected: u64,
    controller_reject_reported: bool,
    magson_client: MagsonTcpClient,
    magson_receiver: Option<Receiver<MagsonSample>>,
    sensor_service: SensorService,
    calculation: CalculationService,
    pid_settings: [PidSettings; 3],
    filter_settings: [FilterSettings; 3],
    calibration: CalibrationSettings,
    pids: [PidController; 3],
    pid_running: [bool; 3],

    /// Rate limit between a commanded field and what the PID actually chases.
    slew: SlewLimiter,
    setpoint_source: SetpointSource,
    setpoint_server: SetpointServer,
    setpoint_receiver: Option<Receiver<[f64; 3]>>,
    last_setpoint_command: Option<Instant>,
    setpoint_port: u16,
    setpoint_bind_address: String,
    profile: Option<SetpointProfile>,
    profile_path: String,
    profile_started: Option<Instant>,
    slew_rate: f64,
    manual_magnitude: f64,
    manual_setpoint_error: Option<String>,

    raw: [f64; 3],
    calibrated: [f64; 3],
    filtered: [f64; 3],
    processed: ProcessedData,
    magson: [f64; 3],
    magson_total: f64,
    outputs: [f64; 3],
    history: PlotHistory,
    follow_plots: bool,
    fullscreen: bool,
    cage: cage::CageView,
    started_at: Instant,
    last_pid_tick: Instant,
    last_handshake: Option<Instant>,
    last_sensor_packet: Option<Instant>,
    last_sensor_packet_wall: Option<SystemTime>,
    /// When a raw count last differed from the one before it, with the counts
    /// that were current then. Raw rather than filtered: the Kalman output
    /// keeps creeping for a while after its input freezes.
    last_sensor_change: Option<Instant>,
    last_sensor_raw: Option<[f64; 3]>,
    /// Commanded field at the previous sensor packet, so the Kalman filter can
    /// be told how far the ramp moved instead of having to discover it.
    /// `None` until the second packet, where the step is unknown, not zero.
    last_filter_setpoint: Option<[f64; 3]>,
    sensor_intended: bool,
    /// Whether the operator wants the controller link up. Drives the same
    /// auto-reconnect the sensor gets: the firmware has no receive timeout, so
    /// a dropped link leaves the coils energised at the last command until
    /// something reopens the port.
    controller_intended: bool,
    last_reconnect: Option<Instant>,
    last_controller_reconnect: Option<Instant>,
    resume_after_reconnect: bool,
    paused_by_watchdog: [bool; 3],
    resume_pending: bool,

    logger: Option<CsvLogger>,
    manual_lat: f64,
    manual_lon: f64,
    manual_result: Option<GeomagnetismResult>,
    manual_error: Option<String>,

    /// "IGRF Model" group: geomagnetic grid map.
    map_grid_path: String,
    map_grid: Option<MapGrid>,
    map_grid_error: Option<String>,
    /// Traced by `igrf_core::contour_segments` whenever a grid loads or
    /// "Generate Model" is pressed - the actual marching-squares math lives
    /// in igrf-core, this just holds the result for rendering.
    map_contours: Option<Vec<ContourSegment>>,
    /// Bumped by "Generate Model" to reset the map plot's pan/zoom, by
    /// changing the `Plot`'s egui id so it reinitialises its view.
    map_view_generation: u64,

    /// "Satellite Position" group: multiple tracked satellites, a ground
    /// station for AOS/LOS, and the simulated clock they share.
    tracked_satellites: Vec<TrackedSat>,
    /// Draft fields for the "add satellite" form; a preset selection copies
    /// straight into these, Manual leaves them for the operator to fill in.
    new_satellite_preset: Option<usize>,
    new_satellite_name: String,
    new_tle_line1: String,
    new_tle_line2: String,
    satellite_tracking: bool,
    sim_time_speed: i32,
    sim_time_offset_s: f64,
    sim_last_tick: Option<Instant>,
    station_lat: f64,
    station_lon: f64,
    elevation_mask_deg: f64,
    satellite_error: Option<String>,
    sat_search: SatSearchState,

    active_tab: AppTab,
    status: String,
    error: Option<String>,
}

impl IgrfApp {
    fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.error = None;
    }

    fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }
}

impl Drop for IgrfApp {
    fn drop(&mut self) {
        let _ = self.zero_outputs();
        self.setpoint_server.disconnect();
        self.sensor_manager.disconnect();
        self.controller_manager.disconnect();
        self.magson_client.disconnect();
        self.magson_receiver = None;
        self.logger = None;
    }
}
