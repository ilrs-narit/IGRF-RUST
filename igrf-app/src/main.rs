mod cage;
mod config_io;
mod connections;
mod control_helpers;
mod geomag;
mod history;
mod logging;
mod netcfg;
mod satellite_tracking;
mod satellite_ui;
mod ui_helpers;

use control_helpers::{
    apply_pid_settings, is_loopback, loop_fault, pid_from_settings, sensor_is_stale, stable_first,
    validate_pid_settings, LoopFault,
};
use history::PlotHistory;
use satellite_ui::{SatSearchState, TrackedSat, OBJECT_TYPE_CHOICES, RCS_OPTIONS};
use ui_helpers::{
    dash_if_blank, error_color, fits_columns, output_fraction, port_selector, show_plot,
    status_pill, LinkState,
};

use eframe::egui::{self, Color32};
use egui_plot::{Legend, Line, Plot, PlotPoint, PlotPoints, Points, Text};
use igrf_core::geomagnetism::GeomagnetismResult;
use igrf_core::satellite::{elevation_deg, PRESETS};
use igrf_core::{
    field_from_magnitude, AppConfig, CalculationService, CalibrationSettings, ContourSegment,
    DisplayMode, FilterSettings, MapGrid, PidController, PidSettings, ProcessedData, SensorService,
    SetpointProfile, SlewLimiter, FIRMWARE_MAX_OUTPUT, NOMINAL_TICK_SECONDS,
};
use igrf_io::{
    list_drives, write_controller_packet, ControllerReplyCounter, CsvLogger, DriveInfo,
    MagsonSample, MagsonTcpClient, SerialPortManager, SetpointServer, StoredTle,
    DEFAULT_BIND_ADDRESS,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const CONFIG_PATH: &str = "SystemConfig.json";
const HANDSHAKE: [u8; 6] = [0x2A, 0x30, 0x30, 0x57, 0x45, 0x0D];
const CONTOUR_LINE_COLOR: Color32 = Color32::WHITE;
const PID_INTERVAL: Duration = Duration::from_millis(100);
const UI_INTERVAL: Duration = Duration::from_millis(50);
const AXES: [char; 3] = ['X', 'Y', 'Z'];
/// A commanded field with no fresh command for this long ramps back to zero.
/// An external propagator that dies would otherwise leave the coils holding
/// its last vector for as long as the app runs.
const SETPOINT_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);
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
    fn new(
        _cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        config_problem: Option<String>,
    ) -> Self {
        let pid_settings = [
            config.pid_x.clone(),
            config.pid_y.clone(),
            config.pid_z.clone(),
        ];
        let filter_settings = [
            config.filter_x.clone(),
            config.filter_y.clone(),
            config.filter_z.clone(),
        ];
        let pids = pid_settings.clone().map(pid_from_settings);
        let initial_setpoint = std::array::from_fn(|axis| pid_settings[axis].setpoint);
        let (available_ports, status) = match serialport::available_ports() {
            Ok(ports) => (stable_first(ports), "Ready".to_owned()),
            Err(error) => (Vec::new(), format!("Port scan unavailable: {error}")),
        };

        let mut app = Self {
            sensor_port: config.sensor_port.clone(),
            sensor_baud: config.sensor_baud,
            controller_port: config.controller_port.clone(),
            controller_baud: config.controller_baud,
            magson_ip: config.sensor2_ip.clone(),
            magson_port: config.sensor2_port as u16,
            log_path: "sensor_log.csv".to_owned(),
            log_files: Vec::new(),
            log_files_status: String::new(),
            log_files_scanned: false,
            log_files_sel: None,
            ext_drives: Vec::new(),
            ext_drive_sel: 0,
            ext_scanned: false,
            ext_cwd: None,
            ext_entries: Vec::new(),
            ext_status: String::new(),
            ext_sel: None,
            transfer_status: String::new(),
            available_ports,
            lan_profiles: netcfg::list_wired().unwrap_or_default(),
            lan_selected: 0,
            lan_cidr: String::new(),
            lan_task: None,
            sensor_manager: SerialPortManager::default(),
            controller_manager: SerialPortManager::default(),
            controller_replies: ControllerReplyCounter::default(),
            controller_sent: 0,
            controller_rejected: 0,
            controller_reject_reported: false,
            magson_client: MagsonTcpClient::default(),
            magson_receiver: None,
            sensor_service: SensorService::with_calibration(config.calibration.clone()),
            calculation: CalculationService::default(),
            pid_settings,
            filter_settings,
            calibration: config.calibration.clone(),
            pids,
            pid_running: [false; 3],
            slew: SlewLimiter::new(config.setpoint_slew_nt_per_second, initial_setpoint),
            setpoint_source: SetpointSource::Manual,
            setpoint_server: SetpointServer::default(),
            setpoint_receiver: None,
            last_setpoint_command: None,
            setpoint_bind_address: config.setpoint_source_bind_address.clone(),
            setpoint_port: if config.setpoint_source_port > 0 {
                config.setpoint_source_port as u16
            } else {
                5005
            },
            profile: None,
            profile_path: config.setpoint_profile_path.clone(),
            profile_started: None,
            slew_rate: config.setpoint_slew_nt_per_second,

            manual_setpoint_error: None,
            raw: [0.0; 3],
            calibrated: [0.0; 3],
            filtered: [0.0; 3],
            processed: ProcessedData::default(),
            magson: [0.0; 3],
            magson_total: 0.0,
            outputs: [0.0; 3],
            history: PlotHistory::default(),
            follow_plots: true,
            // Start from what the config asked for, so the F11 toggle and the
            // `Display.Mode` setting agree on the first press.
            fullscreen: config.display.mode == DisplayMode::Fullscreen,
            cage: cage::CageView::default(),
            started_at: Instant::now(),
            last_pid_tick: Instant::now(),
            last_handshake: None,
            last_sensor_packet: None,
            last_sensor_packet_wall: None,
            last_sensor_change: None,
            last_sensor_raw: None,
            last_filter_setpoint: None,
            sensor_intended: false,
            controller_intended: false,
            last_reconnect: None,
            last_controller_reconnect: None,
            resume_after_reconnect: false,
            paused_by_watchdog: [false; 3],
            resume_pending: false,
            logger: None,

            // Default lat/lon value in manual magnetism calculator to Chiang Mai, Thailand
            manual_lat: 18.8524,
            manual_lon: 98.957478,
            manual_magnitude: 0.0,
            manual_result: None,
            manual_error: None,
            map_grid_path: String::new(),
            map_grid: None,
            map_grid_error: None,
            map_contours: None,
            map_view_generation: 0,
            // An empty config (first run) still gets a ready-to-track example
            // rather than a blank list.
            tracked_satellites: if config.satellites.is_empty() {
                vec![TrackedSat::new(
                    PRESETS[0].name.to_owned(),
                    PRESETS[0].line1.to_owned(),
                    PRESETS[0].line2.to_owned(),
                )]
            } else {
                config
                    .satellites
                    .iter()
                    .map(TrackedSat::from_entry)
                    .collect()
            },
            new_satellite_preset: Some(0),
            new_satellite_name: PRESETS[0].name.to_owned(),
            new_tle_line1: PRESETS[0].line1.to_owned(),
            new_tle_line2: PRESETS[0].line2.to_owned(),
            sat_search: SatSearchState::default(),
            satellite_tracking: false,
            sim_time_speed: 0,
            sim_time_offset_s: 0.0,
            sim_last_tick: None,
            station_lat: config.station_latitude,
            station_lon: config.station_longitude,
            elevation_mask_deg: config.elevation_mask_deg,
            satellite_error: None,
            active_tab: AppTab::default(),
            status,
            error: config_problem,
            config,
        };
        // If a catalog fetch has run before, tle_data.db may hold fresher
        // elements than the config or the presets - fold them in. Local file
        // read only: nothing fetches at startup.
        app.apply_stored_tles();
        app.fill_draft_from_preset(0);
        app.refresh_fetched_types();
        app
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.error = None;
    }

    fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }

    fn refresh_ports(&mut self) {
        match serialport::available_ports() {
            Ok(ports) => {
                self.available_ports = stable_first(ports);
                self.set_status(format!(
                    "Found {} serial port(s)",
                    self.available_ports.len()
                ));
            }
            Err(error) => self.set_error(format!("Cannot list serial ports: {error}")),
        }
    }

    fn refresh_lan(&mut self) {
        match netcfg::list_wired() {
            Ok(profiles) => {
                self.lan_profiles = profiles;
                self.lan_selected = self
                    .lan_selected
                    .min(self.lan_profiles.len().saturating_sub(1));
                if self.lan_cidr.trim().is_empty() {
                    self.lan_cidr = self
                        .lan_profiles
                        .get(self.lan_selected)
                        .map(|profile| profile.addresses.clone())
                        .unwrap_or_default();
                }
            }
            Err(error) => self.set_error(format!("Cannot list LAN profiles: {error}")),
        }
    }

    /// nmcli takes seconds to bring a profile back up, so the work runs off the
    /// UI thread and the result is picked up in `poll_io`.
    fn spawn_lan_task<F>(&mut self, task: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        if self.lan_task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.lan_task = Some(receiver);
        thread::spawn(move || {
            let _ = sender.send(task());
        });
        self.set_status("Applying LAN configuration...");
    }

    fn apply_lan_static(&mut self) {
        let Some(target) = self.lan_profiles.get(self.lan_selected).cloned() else {
            self.set_error("No LAN profile selected");
            return;
        };
        let cidr = self.lan_cidr.trim().to_owned();
        if let Err(error) = netcfg::validate_cidr(&cidr) {
            self.set_error(format!("LAN address: {error}"));
            return;
        }
        self.spawn_lan_task(move || netcfg::apply_static(&target, &cidr));
    }

    fn apply_lan_dhcp(&mut self) {
        let Some(target) = self.lan_profiles.get(self.lan_selected).cloned() else {
            self.set_error("No LAN profile selected");
            return;
        };
        self.spawn_lan_task(move || netcfg::apply_dhcp(&target));
    }

    fn poll_lan_task(&mut self) {
        let Some(receiver) = &self.lan_task else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.lan_task = None;
                match result {
                    Ok(message) => self.set_status(message),
                    Err(error) => self.set_error(format!("LAN: {error}")),
                }
                self.refresh_lan();
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.lan_task = None;
                self.set_error("LAN task ended without a result");
            }
        }
    }

    fn apply_calibration(&mut self) {
        self.calibration.sanitize();
        self.sensor_service.calibration = self.calibration.clone();
    }

    fn apply_filter_settings(&mut self) {
        for (axis, name) in AXES.into_iter().enumerate() {
            let settings = self.filter_settings[axis].clone();
            if self
                .calculation
                .set_noise(axis, settings.q, settings.r)
                .is_err()
                || self
                    .calculation
                    .set_spike_threshold(axis, settings.spike_nt)
                    .is_err()
            {
                self.filter_settings[axis].sanitize();
                self.set_error(format!(
                    "Filter {name}: Q, R and spike must be finite and above zero; restored defaults"
                ));
            }
        }
    }

    fn poll_io(&mut self) {
        self.poll_lan_task();
        self.poll_type_fetch();
        self.apply_filter_settings();
        self.apply_calibration();
        self.maybe_reconnect_sensor();
        self.maybe_reconnect_controller();
        if self.sensor_manager.is_open() {
            let now = Instant::now();
            let should_handshake = self
                .last_handshake
                .map(|sent| now.duration_since(sent) >= Duration::from_millis(500))
                .unwrap_or(true);
            if !self.sensor_manager.parser().is_sensor_ready() && should_handshake {
                self.last_handshake = Some(now);
                if let Err(error) = self.sensor_manager.write(&HANDSHAKE) {
                    self.set_error(format!("Sensor handshake failed: {error}"));
                }
            }

            match self.sensor_manager.read_available() {
                Ok(packets) => {
                    for packet in packets {
                        self.handle_sensor_packet(&packet);
                    }
                }
                Err(error) => {
                    self.set_error(format!("Sensor read failed: {error}"));
                    self.sensor_manager.disconnect();
                }
            }
        }

        self.poll_controller_replies();

        let samples: Vec<MagsonSample> = self
            .magson_receiver
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        for sample in samples {
            self.handle_magson_sample(sample);
        }
        if self.magson_receiver.is_some() && !self.magson_client.is_open() {
            self.magson_receiver = None;
            self.clear_magson();
            self.set_error("Magson connection closed");
        }
    }

    fn handle_sensor_packet(&mut self, packet: &[u8]) {
        // The Kalman filter ticks on sensor packets, not on the PID interval,
        // so its interval and its control input are both measured here.
        let ticks = self
            .last_sensor_packet
            .map(|previous| previous.elapsed().as_secs_f64() / NOMINAL_TICK_SECONDS)
            .unwrap_or(1.0);
        self.last_sensor_packet = Some(Instant::now());
        self.last_sensor_packet_wall = Some(SystemTime::now());
        let calibrated = self.sensor_service.process_data(packet);
        self.raw = [
            self.sensor_service.last_raw_x(),
            self.sensor_service.last_raw_y(),
            self.sensor_service.last_raw_z(),
        ];
        // The first packet starts the clock; after that only a real move
        // restarts it, so an unchanging sensor ages out.
        if self.last_sensor_raw != Some(self.raw) {
            self.last_sensor_raw = Some(self.raw);
            self.last_sensor_change = Some(Instant::now());
        }
        self.calibrated = [calibrated.mag_x, calibrated.mag_y, calibrated.mag_z];
        // `pid_settings[..].setpoint` is where the slew limiter has ramped to,
        // so the difference across two packets is exactly how far the field was
        // asked to move in between - a known input, not something the filter
        // should have to infer from the measurement.
        let setpoint: [f64; 3] = std::array::from_fn(|axis| self.pid_settings[axis].setpoint);
        // Only an axis whose loop is closed and whose packets are reaching the
        // coils has actually been commanded to move. The setpoint keeps ramping
        // while the PID is stopped or the controller is unplugged, and
        // predicting a move that nothing is driving is the same lag with the
        // sign flipped.
        let driven = self.controller_manager.is_open();
        let command_delta = match self.last_filter_setpoint {
            Some(previous) => std::array::from_fn(|axis| {
                if driven && self.pid_running[axis] {
                    setpoint[axis] - previous[axis]
                } else {
                    0.0
                }
            }),
            None => [0.0; 3],
        };
        self.last_filter_setpoint = Some(setpoint);
        self.processed =
            self.calculation
                .process_sensor_data(&calibrated, setpoint, command_delta, ticks);
        self.filtered = [
            self.processed.mag_x,
            self.processed.mag_y,
            self.processed.mag_z,
        ];
        let time = self.started_at.elapsed().as_secs_f64();
        for axis in 0..3 {
            self.history.sensor_setpoint[axis].push(time, self.pid_settings[axis].setpoint);
            self.history.sensor_measured[axis].push(time, self.filtered[axis]);
        }
        let measured_magnitude = self
            .filtered
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        let setpoint_magnitude = self
            .pid_settings
            .iter()
            .map(|settings| settings.setpoint * settings.setpoint)
            .sum::<f64>()
            .sqrt();
        self.history
            .sensor_magnitude_setpoint
            .push(time, setpoint_magnitude);
        self.history
            .sensor_magnitude_measured
            .push(time, measured_magnitude);
    }

    /// Drops the last Magson reading when the link goes away.
    ///
    /// The `Mag2*` CSV columns are written every tick from whatever is in
    /// `self.magson`, so without this a dead link keeps publishing its final
    /// sample for the rest of the run and nothing in the file distinguishes
    /// that from a magnetometer reading a genuinely constant field.
    fn clear_magson(&mut self) {
        self.magson = [0.0; 3];
        self.magson_total = 0.0;
    }

    fn handle_magson_sample(&mut self, sample: MagsonSample) {
        self.magson = [sample.bx, sample.by, sample.bz];
        self.magson_total = self
            .magson
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        let time = self.started_at.elapsed().as_secs_f64();
        for axis in 0..3 {
            self.history.magson[axis].push(time, self.magson[axis]);
        }
        self.history.magson[3].push(time, self.magson_total);
    }

    /// Age of the newest sensor packet; `None` means none has arrived yet.
    ///
    /// `Instant` is CLOCK_MONOTONIC on Linux and does not advance while the
    /// machine is suspended, so waking from a lid-close would look like no time
    /// passed and the watchdog would never fire. The wall clock sees that gap;
    /// taking the larger of the two also keeps an NTP step backwards from
    /// hiding a real stall.
    /// Time since any raw count last moved. `None` before the second packet.
    fn sensor_change_age(&self) -> Option<Duration> {
        Some(self.last_sensor_change?.elapsed())
    }

    fn sensor_age(&self) -> Option<Duration> {
        let monotonic = self.last_sensor_packet?.elapsed();
        let wall = self
            .last_sensor_packet_wall
            .and_then(|at| at.elapsed().ok())
            .unwrap_or(Duration::ZERO);
        Some(monotonic.max(wall))
    }

    /// Commands a field vector through the slew limiter. Nothing in the app
    /// writes `pid_settings[..].setpoint` directly any more: every command,
    /// whatever its source, ramps.
    fn command_setpoint(&mut self, field_nt: [f64; 3]) {
        self.slew.command(field_nt);
    }

    /// Advances the ramp and publishes the result as the live setpoint.
    fn advance_setpoint(&mut self, dt: f64) {
        self.slew.rate_nt_per_second = self.config.setpoint_slew_nt_per_second;
        let current = self.slew.step(dt);
        for (settings, value) in self.pid_settings.iter_mut().zip(current) {
            settings.setpoint = value;
        }
    }

    /// Pulls the newest command from whichever source is live. Only the last
    /// datagram of a burst matters: a setpoint is state, not a queue to drain.
    fn poll_setpoint_source(&mut self) {
        match self.setpoint_source {
            SetpointSource::Manual => {}
            SetpointSource::Profile => {
                let Some(started) = self.profile_started else {
                    return;
                };
                let Some(profile) = &self.profile else {
                    return;
                };
                let time = started.elapsed().as_secs_f64();
                if let Some(field) = profile.sample(time) {
                    self.slew.command(field);
                }
                if time > profile.duration_s() {
                    self.profile_started = None;
                    self.set_status("Setpoint profile finished; holding the last row");
                }
            }
            SetpointSource::Socket => {
                let Some(receiver) = &self.setpoint_receiver else {
                    return;
                };
                if let Some(field) = receiver.try_iter().last() {
                    self.slew.command(field);
                    self.last_setpoint_command = Some(Instant::now());
                    return;
                }
                // A propagator that dies mid-run leaves the cage holding its
                // last command indefinitely. The sensor has a watchdog; the
                // commanded field needs one too.
                if self
                    .last_setpoint_command
                    .is_some_and(|at| at.elapsed() > SETPOINT_SOURCE_TIMEOUT)
                {
                    self.last_setpoint_command = None;
                    self.slew.command([0.0; 3]);
                    self.set_error(format!(
                        "No setpoint datagram for {}s; ramping the field to zero",
                        SETPOINT_SOURCE_TIMEOUT.as_secs()
                    ));
                }
            }
        }
    }

    fn start_setpoint_server(&mut self) {
        if self.setpoint_port == 0 {
            self.set_error("Setpoint port must be greater than zero");
            return;
        }
        let port = self.setpoint_port;
        let address = match self.setpoint_bind_address.trim() {
            "" => DEFAULT_BIND_ADDRESS.to_owned(),
            chosen => chosen.to_owned(),
        };
        match self.setpoint_server.listen(&address, port) {
            Ok(receiver) => {
                self.setpoint_receiver = Some(receiver);
                self.setpoint_source = SetpointSource::Socket;
                self.last_setpoint_command = Some(Instant::now());
                self.set_status(format!(
                    "Setpoint socket listening on UDP {address}:{port}: send \"bx,by,bz\" in nT"
                ));
                if !is_loopback(&address) {
                    self.set_error(format!(
                        "Setpoint socket is reachable from the network on {address}. Datagrams \
                         are not authenticated: any host that can route here can drive the coils."
                    ));
                }
            }
            Err(error) => self.set_error(format!("Setpoint socket failed: {error}")),
        }
    }

    fn stop_setpoint_server(&mut self) {
        self.setpoint_server.disconnect();
        self.setpoint_receiver = None;
        if self.setpoint_source == SetpointSource::Socket {
            self.setpoint_source = SetpointSource::Manual;
        }
        self.set_status("Setpoint socket stopped; holding the last command");
    }

    fn load_setpoint_profile(&mut self) {
        if self.profile_path.trim().is_empty() {
            self.set_error("Setpoint profile path is empty");
            return;
        }
        match SetpointProfile::load(self.profile_path.trim()) {
            Ok(Ok(profile)) => {
                let rows = profile.len();
                let duration = profile.duration_s();
                self.profile = Some(profile);
                self.profile_started = None;
                self.setpoint_source = SetpointSource::Profile;
                self.set_status(format!(
                    "Loaded {rows} profile rows spanning {duration:.1}s; press Play to run"
                ));
            }
            Ok(Err(problem)) => self.set_error(format!("Setpoint profile: {problem}")),
            Err(error) => self.set_error(format!("Cannot read setpoint profile: {error}")),
        }
    }

    /// Applies a magnitude plus the declination/inclination the WMM panel
    /// reports, so a run can be commanded as "the local field at 1.2x" instead
    /// of three hand-computed components.
    fn apply_manual_magnitude(&mut self) {
        let result = (|| {
            let magnitude = self.manual_magnitude;
            if !magnitude.is_finite() || magnitude < 0.0 {
                return Err("magnitude must be a non-negative number".to_owned());
            }
            let wmm = self
                .manual_result
                .ok_or_else(|| "run the WMM2025 calculation first".to_owned())?;
            Ok::<_, String>(field_from_magnitude(
                magnitude,
                wmm.declination,
                wmm.inclination,
            ))
        })();
        match result {
            Ok(field) => {
                self.setpoint_source = SetpointSource::Manual;
                self.manual_setpoint_error = None;
                self.command_setpoint(field);
                self.set_status(format!(
                    "Commanded |B| {:.1} nT along the WMM direction; ramping at {:.0} nT/s",
                    self.manual_magnitude, self.config.setpoint_slew_nt_per_second
                ));
            }
            Err(problem) => self.manual_setpoint_error = Some(problem),
        }
    }

    fn run_pid(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_pid_tick);
        if elapsed < PID_INTERVAL {
            return;
        }
        self.last_pid_tick = now;
        // The repaint that drives this loop is quantised to the display's
        // refresh, so a nominal 100 ms tick lands anywhere from 100 to 133 ms.
        // Feeding the real interval to the PID keeps Ki and Kd meaning what
        // they meant when they were tuned.
        let dt = elapsed.as_secs_f64();
        self.poll_setpoint_source();
        self.advance_setpoint(dt);
        if let Some((axis, error)) =
            self.pid_settings
                .iter()
                .enumerate()
                .find_map(|(axis, settings)| {
                    validate_pid_settings(settings)
                        .err()
                        .map(|error| (axis, error))
                })
        {
            self.pid_running = [false; 3];
            let _ = self.zero_outputs();
            self.pid_settings[axis].sanitize();
            self.set_error(format!(
                "PID {}: {error}; restored safe defaults",
                AXES[axis]
            ));
            self.log_snapshot(elapsed);
            return;
        }
        let fault = loop_fault(
            self.controller_manager.is_open(),
            self.sensor_age(),
            self.sensor_change_age(),
        );
        if self.pid_running.iter().any(|state| *state) {
            if let Some(fault) = fault {
                let reason = match fault {
                    LoopFault::ControllerDown => {
                        "controller link down; the coils hold their last command until it reopens"
                            .to_owned()
                    }
                    LoopFault::SensorFrozen => format!(
                        "sensor readings unchanged for {:.1}s",
                        self.sensor_change_age().unwrap_or_default().as_secs_f64()
                    ),
                    LoopFault::SensorStale => match self.sensor_age() {
                        Some(age) => format!("no sensor data for {:.1}s", age.as_secs_f64()),
                        None => "no sensor data received yet".to_owned(),
                    },
                };
                self.paused_by_watchdog = self.pid_running;
                self.watchdog_pause();
                self.set_error(format!("PID paused: {reason}"));
                // The rows around a fault are the ones worth having; the early
                // return would drop exactly those from the CSV.
                self.log_snapshot(elapsed);
                return;
            }
        }

        // A reconnect only proves a port reopened, not that the link behind it
        // works, so every fault has to be clear before the coils are driven.
        if self.resume_pending && fault.is_none() {
            self.resume_pending = false;
            self.pid_running = self.paused_by_watchdog;
            self.paused_by_watchdog = [false; 3];
            if self.pid_running.iter().any(|state| *state) {
                self.set_status("Links back; PID resumed");
            }
        }

        for axis in 0..3 {
            apply_pid_settings(&mut self.pids[axis], &self.pid_settings[axis]);
            self.outputs[axis] = if self.pid_running[axis] {
                self.pids[axis].calculate_dt(
                    self.pid_settings[axis].setpoint,
                    self.filtered[axis],
                    dt,
                )
            } else {
                0.0
            };
        }

        self.write_outputs();
        self.log_snapshot(elapsed);
    }

    /// Sends the current outputs to the controller. A failed write closes the
    /// port: the link is gone, and pretending otherwise would leave the coils
    /// holding the last packet with nothing watching them.
    fn write_outputs(&mut self) {
        if !self.controller_manager.is_open() {
            return;
        }
        if let Err(error) = write_controller_packet(
            &mut self.controller_manager,
            self.outputs[0],
            self.outputs[1],
            self.outputs[2],
        ) {
            self.set_error(format!("Controller write failed: {error}"));
            self.controller_manager.disconnect();
        } else {
            self.controller_sent = self.controller_sent.saturating_add(1);
        }
    }

    fn reset_controller_link_stats(&mut self) {
        self.controller_replies.reset();
        self.controller_sent = 0;
        self.controller_rejected = 0;
        self.controller_reject_reported = false;
    }

    /// Drains the controller's return path.
    ///
    /// The firmware answers only when it throws a packet away, so anything read
    /// here is a command the coils never acted on. A silent link is a healthy
    /// one; a growing count means the cable, not the control law, is the
    /// problem.
    fn poll_controller_replies(&mut self) {
        if !self.controller_manager.is_open() {
            return;
        }
        match self.controller_manager.read_raw() {
            Ok(bytes) => {
                let rejected = self.controller_replies.feed(&bytes) as u64;
                if rejected == 0 {
                    return;
                }
                self.controller_rejected = self.controller_rejected.saturating_add(rejected);
                // Once per connection: at 10 Hz a bad cable would otherwise
                // overwrite the status line with nothing else.
                if !self.controller_reject_reported {
                    self.controller_reject_reported = true;
                    self.set_error(
                        "Controller rejected a packet (CRC). Watch the reject count on the \
                         controller panel.",
                    );
                }
            }
            Err(error) => {
                self.set_error(format!("Controller read failed: {error}"));
                self.controller_manager.disconnect();
            }
        }
    }

    fn reset_axis(&mut self, axis: usize) {
        self.pids[axis].reset();
        match axis {
            0 => self.calculation.reset_filter_x(),
            1 => self.calculation.reset_filter_y(),
            _ => self.calculation.reset_filter_z(),
        }
        self.outputs[axis] = 0.0;
        self.set_status(format!("Reset axis {} PID/filter", AXES[axis]));
    }

    fn master_reset(&mut self) {
        self.stop_all();
        for pid in &mut self.pids {
            pid.reset();
        }
        self.calculation.reset_filters();
        // Leaving a commanded ramp in flight would have the cage climb back
        // toward the old target the moment an axis is started again.
        self.slew.snap([0.0; 3]);
        for settings in &mut self.pid_settings {
            settings.setpoint = 0.0;
        }
        self.profile_started = None;
        self.outputs = [0.0; 3];
        self.filtered = [0.0; 3];
        self.processed = ProcessedData::default();
        self.history.clear();
        if self.error.is_none() {
            self.set_status("Master reset complete");
        }
    }

    fn stop_all(&mut self) {
        self.pid_running = [false; 3];
        // A deliberate stop clears the loop: whoever presses this wants the
        // cage inert, not parked ready to resume.
        for pid in &mut self.pids {
            pid.reset();
        }
        self.paused_by_watchdog = [false; 3];
        self.resume_pending = false;
        if let Err(error) = self.zero_outputs() {
            self.set_error(format!("STOP ALL: controller write failed: {error}"));
            return;
        }
        self.set_status("STOP ALL: every axis paused, outputs zeroed");
    }

    /// Watchdog pause: coils to zero, but the loop keeps its integral.
    ///
    /// Nulling the ambient field puts nearly the whole output in the integral
    /// term. Clearing it on a one-second sensor dropout means the error jumps
    /// to the full ambient field on resume and the rebuilt integral overshoots
    /// into the output limit - a full-scale transient through the 48 V drivers
    /// every time the USB link hiccups. Holding it makes the resume bumpless.
    fn watchdog_pause(&mut self) {
        self.pid_running = [false; 3];
        for pid in &mut self.pids {
            pid.hold();
        }
        if let Err(error) = self.zero_outputs() {
            self.set_error(format!("Watchdog pause: controller write failed: {error}"));
        }
    }

    /// Top-level page navigation
    fn show_tab_strip(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for tab in [AppTab::Control, AppTab::Model, AppTab::Settings] {
                let selected = self.active_tab == tab;
                let button =
                    egui::Button::new(egui::RichText::new(tab.label()).strong().size(15.0))
                        .selected(selected)
                        .min_size(egui::vec2(120.0, 28.0));
                if ui.add(button).clicked() {
                    self.active_tab = tab;
                }
            }
        });
        ui.add_space(2.0);
        ui.separator();
    }

    fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("IGRF control");
            ui.separator();
            let sensor_age = self.sensor_age();
            status_pill(
                ui,
                "Sensor",
                if !self.sensor_manager.is_open() {
                    LinkState::Off
                } else if self.sensor_manager.parser().is_sensor_ready()
                    && !sensor_is_stale(sensor_age)
                {
                    LinkState::On
                } else {
                    LinkState::Wait
                },
            );
            if self.sensor_manager.is_open() {
                ui.label(
                    egui::RichText::new(match sensor_age {
                        Some(age) => format!("{:.1}s", age.as_secs_f64()),
                        None => "no data".to_owned(),
                    })
                    .small()
                    .weak(),
                );
            }
            status_pill(
                ui,
                "Controller",
                LinkState::from_open(self.controller_manager.is_open()),
            );
            status_pill(
                ui,
                "Magson",
                LinkState::from_open(self.magson_client.is_open()),
            );
            status_pill(ui, "CSV", LinkState::from_open(self.logger.is_some()));
            ui.separator();
            let running = self.pid_running.iter().filter(|state| **state).count();
            ui.label(format!("PID {running}/3 running"));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let stop = egui::Button::new(
                    egui::RichText::new("STOP ALL")
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(STOP_RED)
                .min_size(egui::vec2(110.0, 26.0));
                if ui.add(stop).clicked() {
                    self.stop_all();
                }
                if ui.button("Master reset").clicked() {
                    self.master_reset();
                }
                // In borderless fullscreen there is no title-bar close button,
                // so the app has to offer its own exit. Closing runs `Drop`,
                // which zeroes the coils before the process goes away.
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn show_connection_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh ports").clicked() {
                self.refresh_ports();
            }
            ui.label(format!("{} detected", self.available_ports.len()));
        });
        ui.separator();
        ui.label("Sensor serial");
        port_selector(
            ui,
            "sensor-port",
            &mut self.sensor_port,
            &self.available_ports,
        );
        ui.horizontal(|ui| {
            ui.label("Baud");
            ui.add(
                egui::DragValue::new(&mut self.sensor_baud)
                    .speed(100.0)
                    .range(1..=u32::MAX),
            );
            if ui.button("Connect").clicked() {
                self.connect_sensor();
            }
            if ui.button("Disconnect").clicked() {
                self.disconnect_sensor();
            }
        });
        ui.label(if self.sensor_manager.is_open() {
            if self.sensor_manager.parser().is_sensor_ready() {
                "Sensor: connected / ready"
            } else {
                "Sensor: connected / waiting for OK"
            }
        } else {
            "Sensor: disconnected"
        });
        ui.checkbox(
            &mut self.resume_after_reconnect,
            "Resume PID after auto-reconnect",
        )
        .on_hover_text(
            "Off: the loop stays paused until someone starts it. On: it restarts \
             the coils by itself once packets return - only for unattended runs.",
        );

        ui.separator();
        ui.label("Controller serial");
        port_selector(
            ui,
            "controller-port",
            &mut self.controller_port,
            &self.available_ports,
        );
        ui.horizontal(|ui| {
            ui.label("Baud");
            ui.add(
                egui::DragValue::new(&mut self.controller_baud)
                    .speed(100.0)
                    .range(1..=u32::MAX),
            );
            if ui.button("Connect").clicked() {
                self.connect_controller();
            }
            if ui.button("Disconnect").clicked() {
                self.disconnect_controller();
            }
        });
        ui.label(if self.controller_manager.is_open() {
            "Controller: connected"
        } else {
            "Controller: disconnected"
        });
        // Kept visible after a disconnect: a link bad enough to drop packets is
        // a link bad enough to drop entirely, and the count is the evidence.
        if self.controller_sent > 0 {
            let rate = 100.0 * self.controller_rejected as f64 / self.controller_sent as f64;
            let label = format!(
                "Rejected: {} / {} sent ({rate:.2}%)",
                self.controller_rejected, self.controller_sent
            );
            if self.controller_rejected == 0 {
                ui.label(label)
            } else {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 60), label)
            }
            .on_hover_text(
                "Packets the firmware answered with \"Error\\r\" because the CRC did not \
                 match. Those commands never reached the coils. Anything above zero is a \
                 cabling or baud problem, not a tuning one.",
            );
        }

        ui.separator();
        ui.label("Magson TCP");
        ui.horizontal(|ui| {
            ui.label("IP/host");
            ui.add(egui::TextEdit::singleline(&mut self.magson_ip).desired_width(120.0));
        });
        ui.horizontal(|ui| {
            ui.label("Port");
            ui.add(
                egui::DragValue::new(&mut self.magson_port)
                    .speed(1.0)
                    .range(1..=u16::MAX),
            );
            if ui.button("Connect").clicked() {
                self.connect_magson();
            }
            if ui.button("Disconnect").clicked() {
                self.disconnect_magson();
            }
        });
        ui.label(if self.magson_client.is_open() {
            "Magson: connected"
        } else {
            "Magson: disconnected"
        });
        // The frame layout is not confirmed (see the README), so a climbing
        // count is the difference between "this build ignores some types" and
        // "the stream is not being understood at all".
        let dropped = self.magson_client.dropped_frames();
        if dropped > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(220, 120, 60),
                format!("Undecoded frames: {dropped}"),
            )
            .on_hover_text(
                "Frames read but not decoded: types this build ignores, plus                  anything discarded while resynchronising after a lost byte.",
            );
        }
    }

    fn show_lan_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.refresh_lan();
            }
            if self.lan_task.is_some() {
                ui.spinner();
                ui.label("applying...");
            }
        });
        if self.lan_profiles.is_empty() {
            ui.label("No wired NetworkManager profile found");
            return;
        }

        let selected = self.lan_selected.min(self.lan_profiles.len() - 1);
        self.lan_selected = selected;
        let labels: Vec<String> = self
            .lan_profiles
            .iter()
            .map(|profile| profile.label())
            .collect();
        egui::ComboBox::from_id_salt("lan-profile")
            .selected_text(labels[selected].clone())
            .show_ui(ui, |ui| {
                for (index, label) in labels.iter().enumerate() {
                    ui.selectable_value(&mut self.lan_selected, index, label);
                }
            });

        let target = self.lan_profiles[self.lan_selected].clone();
        ui.label(
            egui::RichText::new(format!(
                "now: {} {}",
                target.method,
                if target.addresses.is_empty() {
                    "--"
                } else {
                    &target.addresses
                }
            ))
            .small()
            .weak(),
        );
        if target.carries_default_route {
            ui.colored_label(
                Color32::LIGHT_RED,
                "carries the default route - locked to avoid cutting this machine off",
            );
            return;
        }

        ui.horizontal(|ui| {
            ui.label("Address");
            ui.add(egui::TextEdit::singleline(&mut self.lan_cidr).desired_width(130.0));
        });
        let busy = self.lan_task.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Apply static"))
                .clicked()
            {
                self.apply_lan_static();
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Use DHCP"))
                .clicked()
            {
                self.apply_lan_dhcp();
            }
        });
    }

    fn show_config_panel(&mut self, ui: &mut egui::Ui) {
        if ui.button("Load SystemConfig.json").clicked() {
            self.load_config();
        }
        if ui.button("Save SystemConfig.json").clicked() {
            self.save_config();
        }
        ui.label("CSV path");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.log_path).desired_width(150.0));
            if self.logger.is_some() {
                if ui.button("Stop").clicked() {
                    self.stop_logging();
                }
            } else if ui.button("Start").clicked() {
                self.start_logging();
            }
        });
        ui.label(if self.logger.is_some() {
            "CSV: logging"
        } else {
            "CSV: stopped"
        });
    }

    // Get the logs directory
    fn log_directory(&self) -> PathBuf {
        match Path::new(self.log_path.trim()).parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("logs"),
        }
    }

    /// Rescan the log directory for the "Saved log files" list. Read-only: a
    /// missing directory is reported, not created.
    fn refresh_log_files(&mut self) {
        let dir = self.log_directory();
        self.log_files.clear();
        self.log_files_scanned = true;
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    match entry.metadata() {
                        Ok(meta) if meta.is_file() => self.log_files.push(FileRow {
                            name: entry.file_name().to_string_lossy().into_owned(),
                            is_dir: false,
                            size_bytes: meta.len(),
                        }),
                        _ => {}
                    }
                }
                // Sort files by name descending
                self.log_files.sort_by(|a, b| b.name.cmp(&a.name));
                self.log_files_status = format!("{} item(s)", self.log_files.len());
                prune_selection(&mut self.log_files_sel, &self.log_files);
            }
            Err(error) => {
                self.log_files_status = format!("Cannot read {}: {error}", dir.display());
            }
        }
    }
    // Show log files panel: left column is the logs folder, right column is the external drive
    fn show_log_files_panel(&mut self, ui: &mut egui::Ui) {
        if !self.log_files_scanned {
            self.refresh_log_files();
        }
        if !self.ext_scanned {
            self.refresh_drives();
            self.ext_scanned = true;
        }
        let logs_path = self.log_directory().display().to_string();
        let drive_path = self
            .ext_cwd
            .as_deref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "(no drive selected)".to_owned());

        ui.horizontal_top(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let mid = 54.0;
            let side = ((ui.available_width() - mid - gap * 2.0) / 2.0).max(140.0);

            ui.allocate_ui_with_layout(
                egui::vec2(side, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(side);
                    ui.strong("Logs folder");
                    ui.label(egui::RichText::new(&logs_path).monospace().small());
                    self.show_logs_folder_list(ui);
                },
            );

            // Middle strip, between the two lists
            ui.allocate_ui_with_layout(
                egui::vec2(mid, 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_width(mid);
                    self.show_transfer_buttons(ui);
                },
            );

            ui.allocate_ui_with_layout(
                egui::vec2(side, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(side);
                    ui.strong("External drive");
                    ui.label(egui::RichText::new(&drive_path).monospace().small());
                    self.show_external_drive_panel(ui);
                },
            );
        });

        if !self.transfer_status.is_empty() {
            ui.label(egui::RichText::new(&self.transfer_status).small().weak());
        }
    }

    /// The copy buttons in the strip between the two lists: '>>' and '<<'
    fn show_transfer_buttons(&mut self, ui: &mut egui::Ui) {
        let to_logs_ready = self.ext_sel.is_some();
        let to_drive_ready = self.log_files_sel.is_some() && self.ext_cwd.is_some();
        let size = egui::vec2(40.0, 30.0);

        ui.add_space(96.0);
        if ui
            .add_enabled(to_drive_ready, egui::Button::new(">>").min_size(size))
            .on_hover_text("Copy the selected log file into the open drive folder")
            .clicked()
        {
            self.copy_logs_to_drive();
        }
        ui.add_space(8.0);
        if ui
            .add_enabled(to_logs_ready, egui::Button::new("<<").min_size(size))
            .on_hover_text("Copy the selected drive file into the logs folder")
            .clicked()
        {
            self.copy_drive_to_logs();
        }
    }

    fn copy_drive_to_logs(&mut self) {
        let (Some(name), Some(cwd)) = (self.ext_sel.clone(), self.ext_cwd.clone()) else {
            return;
        };
        let dest = self.log_directory();
        self.copy_into(cwd.join(name), dest);
    }

    fn copy_logs_to_drive(&mut self) {
        let (Some(name), Some(cwd)) = (self.log_files_sel.clone(), self.ext_cwd.clone()) else {
            return;
        };
        let source = self.log_directory().join(name);
        self.copy_into(source, cwd);
    }

    /// Copy `source` into `dest_dir`, keeping its file name. Overwrites a file of
    /// the same name. Both lists are rescanned afterwards so the new file shows up.
    fn copy_into(&mut self, source: PathBuf, dest_dir: PathBuf) {
        let Some(name) = source.file_name().map(|name| name.to_owned()) else {
            self.transfer_status = "No file selected".to_owned();
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&dest_dir) {
            self.transfer_status = format!("Cannot open {}: {error}", dest_dir.display());
            return;
        }
        let dest = dest_dir.join(&name);
        let replaced = dest.exists();
        match std::fs::copy(&source, &dest) {
            Ok(bytes) => {
                self.transfer_status = format!(
                    "{} {} ({}) to {}",
                    if replaced { "Replaced" } else { "Copied" },
                    name.to_string_lossy(),
                    human_size(bytes),
                    dest_dir.display()
                );
            }
            Err(error) => {
                self.transfer_status = format!("Copy failed: {error}");
            }
        }
        self.refresh_log_files();
        self.refresh_ext_files();
    }

    /// Left column: ls every file in the log directory, with a Refresh to
    /// rescan. Click a file to select it for a `>>` copy.
    fn show_logs_folder_list(&mut self, ui: &mut egui::Ui) {
        if ui.button("Refresh").clicked() {
            self.refresh_log_files();
        }
        ui.label(egui::RichText::new(&self.log_files_status).small().weak());
        if self.log_files.is_empty() {
            return;
        }
        let mut pick: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("logs-folder-scroll")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("saved-log-files")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        for row in &self.log_files {
                            let selected = self.log_files_sel.as_deref() == Some(row.name.as_str());
                            if ui
                                .selectable_label(
                                    selected,
                                    egui::RichText::new(&row.name).monospace(),
                                )
                                .clicked()
                            {
                                pick = Some(row.name.clone());
                            }
                            ui.label(
                                egui::RichText::new(human_size(row.size_bytes))
                                    .small()
                                    .weak(),
                            );
                            ui.end_row();
                        }
                    });
            });
        if let Some(name) = pick {
            self.log_files_sel =
                (self.log_files_sel.as_deref() != Some(name.as_str())).then_some(name);
        }
    }

    /// Right column: pick a connected drive from the dropdown
    fn show_external_drive_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("Rescan")
                .on_hover_text("Look for newly connected drives")
                .clicked()
            {
                self.refresh_drives();
            }
            let selected_text = self
                .ext_drives
                .get(self.ext_drive_sel)
                .map(|drive| drive.label.clone())
                .unwrap_or_else(|| "(no drive)".to_owned());
            let mut pick: Option<usize> = None;
            egui::ComboBox::from_id_salt("ext-drive")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for (index, drive) in self.ext_drives.iter().enumerate() {
                        if ui
                            .selectable_label(index == self.ext_drive_sel, &drive.label)
                            .clicked()
                        {
                            pick = Some(index);
                        }
                    }
                });
            if let Some(index) = pick {
                self.select_ext_drive(index);
            }
        });

        if self.ext_drives.is_empty() {
            ui.label(
                egui::RichText::new("No drives detected - plug one in and Rescan")
                    .small()
                    .weak(),
            );
            return;
        }
        if self.ext_cwd.is_none() {
            ui.label(egui::RichText::new("Pick a drive above").small().weak());
            return;
        }

        ui.horizontal(|ui| {
            let at_root = self.ext_cwd.as_deref().and_then(Path::parent).is_none();
            if ui
                .add_enabled(!at_root, egui::Button::new("Back"))
                .clicked()
            {
                if let Some(parent) = self.ext_cwd.as_deref().and_then(Path::parent) {
                    self.navigate_ext(parent.to_path_buf());
                }
            }
            if ui.button("Refresh").clicked() {
                self.refresh_ext_files();
            }
        });
        ui.label(egui::RichText::new(&self.ext_status).small().weak());

        let mut into: Option<PathBuf> = None;
        let mut pick: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("ext-drive-scroll")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("ext-drive-files")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        for row in &self.ext_entries {
                            if row.is_dir {
                                if ui
                                    .selectable_label(false, format!("[dir]  {}", row.name))
                                    .clicked()
                                {
                                    if let Some(cwd) = &self.ext_cwd {
                                        into = Some(cwd.join(&row.name));
                                    }
                                }
                                ui.label("");
                            } else {
                                let selected = self.ext_sel.as_deref() == Some(row.name.as_str());
                                if ui
                                    .selectable_label(
                                        selected,
                                        egui::RichText::new(&row.name).monospace(),
                                    )
                                    .clicked()
                                {
                                    pick = Some(row.name.clone());
                                }
                                ui.label(
                                    egui::RichText::new(human_size(row.size_bytes))
                                        .small()
                                        .weak(),
                                );
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(path) = into {
            self.navigate_ext(path);
        } else if let Some(name) = pick {
            self.ext_sel = (self.ext_sel.as_deref() != Some(name.as_str())).then_some(name);
        }
    }

    /// Rebuild the connected-drive list for the dropdown, keeping the current
    /// selection pointed at the same root when it is still present.
    fn refresh_drives(&mut self) {
        let previous = self
            .ext_drives
            .get(self.ext_drive_sel)
            .map(|drive| drive.root.clone());
        self.ext_drives = list_drives();
        self.ext_drive_sel = previous
            .and_then(|root| self.ext_drives.iter().position(|drive| drive.root == root))
            .unwrap_or(0);
        // Drop a browse position that belonged to a drive now unplugged.
        if let Some(cwd) = self.ext_cwd.clone() {
            if !self
                .ext_drives
                .iter()
                .any(|drive| cwd.starts_with(&drive.root))
            {
                self.ext_cwd = None;
                self.ext_entries.clear();
                self.ext_status.clear();
                self.ext_sel = None;
            }
        }
    }

    /// Point the browser at a drive's root and list it.
    fn select_ext_drive(&mut self, index: usize) {
        self.ext_drive_sel = index;
        if let Some(drive) = self.ext_drives.get(index) {
            self.ext_cwd = Some(PathBuf::from(&drive.root));
            self.ext_sel = None;
            self.refresh_ext_files();
        }
    }

    /// Move the browser to `dir` and list it.
    fn navigate_ext(&mut self, dir: PathBuf) {
        self.ext_cwd = Some(dir);
        self.ext_sel = None;
        self.refresh_ext_files();
    }

    /// List the current external directory: folders first, then files by name.
    fn refresh_ext_files(&mut self) {
        self.ext_entries.clear();
        let Some(dir) = self.ext_cwd.clone() else {
            self.ext_status = "No drive selected".to_owned();
            return;
        };
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let Ok(meta) = entry.metadata() else { continue };
                    let is_dir = meta.is_dir();
                    if !is_dir && !meta.is_file() {
                        continue;
                    }
                    self.ext_entries.push(FileRow {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        is_dir,
                        size_bytes: if is_dir { 0 } else { meta.len() },
                    });
                }
                self.ext_entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                self.ext_status = format!("{} item(s)", self.ext_entries.len());
                prune_selection(&mut self.ext_sel, &self.ext_entries);
            }
            Err(error) => {
                self.ext_status = format!("Cannot read {}: {error}", dir.display());
            }
        }
    }

    /// Show display panel: window/fullscreen, UI scale, and which monitor to use for fullscreen.
    fn show_display_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Window mode (applies on restart)").weak());
        ui.horizontal(|ui| {
            for mode in [DisplayMode::Window, DisplayMode::Fullscreen] {
                let label = match mode {
                    DisplayMode::Window => "Window",
                    DisplayMode::Fullscreen => "Fullscreen borderless",
                };
                if ui
                    .selectable_label(self.config.display.mode == mode, label)
                    .clicked()
                {
                    self.config.display.mode = mode;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("UI scale");
            ui.add(
                egui::DragValue::new(&mut self.config.display.ui_scale)
                    .speed(0.05)
                    .range(0.5..=3.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Fullscreen monitor");
            ui.add(
                egui::DragValue::new(&mut self.config.display.fullscreen_monitor)
                    .speed(0.1)
                    .range(0..=16),
            );
        });
        ui.label(
            egui::RichText::new(
                "Fullscreen with a UI scale above 1.0 suits an embedded 1024x600 touchscreen. \
                 Monitor 0 is the only screen on a single-panel kiosk.",
            )
            .small()
            .weak(),
        );
    }

    /// Setpoint command panel: where the field comes from and how fast it may
    /// change. Everything here is nanotesla.
    fn show_setpoint_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Source");
            for source in [
                SetpointSource::Manual,
                SetpointSource::Profile,
                SetpointSource::Socket,
            ] {
                if ui
                    .selectable_label(self.setpoint_source == source, source.label())
                    .clicked()
                {
                    self.setpoint_source = source;
                }
            }
        });

        let target = self.slew.target();
        let current = self.slew.current();
        let magnitude = |value: [f64; 3]| value.iter().map(|v| v * v).sum::<f64>().sqrt();
        ui.label(
            egui::RichText::new(format!(
                "now {:.1} nT -> target {:.1} nT",
                magnitude(current),
                magnitude(target)
            ))
            .monospace(),
        );
        if !self.slew.is_settled() {
            ui.label(egui::RichText::new("ramping").small().weak());
        }

        ui.horizontal(|ui| {
            ui.label("Slew nT/s");
            let response = ui.add(
                egui::DragValue::new(&mut self.slew_rate)
                    .speed(100.0)
                    .range(1.0..=1e6),
            );
            if response.changed() {
                self.config.setpoint_slew_nt_per_second = self.slew_rate;
                self.set_status(format!("Setpoint ramps at {:.0} nT/s", self.slew_rate));
            }
        });

        match self.setpoint_source {
            SetpointSource::Manual => {
                ui.separator();
                ui.label("Command |B| along the WMM direction");
                ui.horizontal(|ui| {
                    ui.label("|B| nT");
                    ui.add(
                        egui::DragValue::new(&mut self.manual_magnitude)
                            .speed(1000.0)
                            .range(0.0..=1e6),
                    );
                    if ui.button("Command").clicked() {
                        self.apply_manual_magnitude();
                    }
                });
                match self.manual_result {
                    Some(wmm) => ui.label(
                        egui::RichText::new(format!(
                            "direction D {:.2} deg, I {:.2} deg",
                            wmm.declination, wmm.inclination
                        ))
                        .small()
                        .weak(),
                    ),
                    None => ui.label(
                        egui::RichText::new("run the WMM2025 calculation for a direction")
                            .small()
                            .weak(),
                    ),
                };
                if let Some(error) = &self.manual_setpoint_error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                ui.label(
                    egui::RichText::new("per-axis setpoints stay editable on each axis card")
                        .small()
                        .weak(),
                );
            }
            SetpointSource::Profile => {
                ui.separator();
                ui.label("CSV: time_s,bx_nt,by_nt,bz_nt");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.profile_path).desired_width(150.0));
                    if ui.button("Load").clicked() {
                        self.load_setpoint_profile();
                    }
                });
                match &self.profile {
                    Some(profile) => {
                        let rows = profile.len();
                        let duration = profile.duration_s();
                        ui.label(
                            egui::RichText::new(format!("{rows} rows, {duration:.1}s"))
                                .small()
                                .weak(),
                        );
                        ui.horizontal(|ui| {
                            let running = self.profile_started.is_some();
                            if ui.button(if running { "Stop" } else { "Play" }).clicked() {
                                self.profile_started = (!running).then(Instant::now);
                            }
                            if let Some(started) = self.profile_started {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "t = {:.1}s",
                                        started.elapsed().as_secs_f64()
                                    ))
                                    .monospace(),
                                );
                            }
                        });
                    }
                    None => {
                        ui.label(egui::RichText::new("no profile loaded").small().weak());
                    }
                }
            }
            SetpointSource::Socket => {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("UDP port");
                    ui.add(
                        egui::DragValue::new(&mut self.setpoint_port)
                            .speed(1.0)
                            .range(1..=u16::MAX),
                    );
                    ui.label("Bind");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.setpoint_bind_address)
                            .desired_width(96.0),
                    )
                    .on_hover_text(
                        "Interface the listener accepts datagrams on. 127.0.0.1 keeps it on \
                         this machine. Anything else lets any host that can route here drive \
                         the coils, with no authentication.",
                    );
                    if self.setpoint_server.is_listening() {
                        if ui.button("Stop").clicked() {
                            self.stop_setpoint_server();
                        }
                    } else if ui.button("Listen").clicked() {
                        self.start_setpoint_server();
                    }
                });
                ui.label(
                    egui::RichText::new(if self.setpoint_server.is_listening() {
                        "listening - send \"bx,by,bz\" in nT, newest datagram wins"
                    } else {
                        "stopped"
                    })
                    .small()
                    .weak(),
                );
            }
        }
    }

    /// Sensor calibration. These belong to the physical unit in the cage, so
    /// re-fitting the ellipsoid must not need a rebuild.
    fn show_calibration_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("nT/count");
            ui.add(
                egui::DragValue::new(&mut self.calibration.count_to_nt)
                    .speed(0.001)
                    .range(1e-6..=1e6),
            );
        });
        ui.label(egui::RichText::new("Hard iron nT").small().weak());
        egui::Grid::new("hard-iron-grid")
            .num_columns(3)
            .show(ui, |ui| {
                for value in &mut self.calibration.hard_iron {
                    ui.add(egui::DragValue::new(value).speed(1.0));
                }
                ui.end_row();
            });
        ui.label(egui::RichText::new("Soft iron").small().weak());
        egui::Grid::new("soft-iron-grid")
            .num_columns(3)
            .show(ui, |ui| {
                for row in &mut self.calibration.soft_iron {
                    for value in row {
                        ui.add(egui::DragValue::new(value).speed(0.0001).max_decimals(6));
                    }
                    ui.end_row();
                }
            });
        for warning in [self.calibration_warning(), self.authority_warning()]
            .into_iter()
            .flatten()
        {
            ui.colored_label(Color32::LIGHT_RED, warning);
        }
        ui.label(
            egui::RichText::new("Save SystemConfig.json to keep these")
                .small()
                .weak(),
        );
    }

    fn show_manual_panel(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("manual-wmm-grid")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Latitude");
                ui.add(
                    egui::DragValue::new(&mut self.manual_lat)
                        .speed(0.01)
                        .range(-90.0..=90.0),
                );
                ui.end_row();
                ui.label("Longitude");
                ui.add(
                    egui::DragValue::new(&mut self.manual_lon)
                        .speed(0.01)
                        .range(-180.0..=180.0),
                );
                ui.end_row();
            });
        if ui.button("Calculate Magnetism").clicked() {
            self.calculate_manual_wmm();
        }
        if let Some(error) = &self.manual_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(result) = self.manual_result {
            egui::Grid::new("manual-wmm-result")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Declination", result.declination),
                        ("Inclination", result.inclination),
                        ("Horizontal Intensity", result.horizontal_intensity),
                        ("Total Intensity", result.total_intensity),
                        ("X", result.x),
                        ("Y", result.y),
                        ("Z", result.z),
                    ] {
                        ui.label(label);
                        ui.label(format!("{value:.4}"));
                        ui.end_row();
                    }
                });
        }
    }

    /// Tab left side: IGRF Model group loaded from a text file, and a "Generate Model"
    fn show_map_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Load Model").clicked() {
                self.browse_map_grid();
            }
        });
        if !self.map_grid_path.is_empty() {
            ui.label(format!("File: {}", self.map_grid_path));
        }
        if let Some(error) = &self.map_grid_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    /// Time group: live UTC clock
    fn show_time_panel(&self, ui: &mut egui::Ui) {
        ui.label(format!(
            "UTC: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")
        ));
    }

    /// Satellite Position group: SGP4 propagation and the TEME->geodetic
    /// conversion happen in `igrf_core::satellite`; this owns the tracked
    /// satellite list, the "add satellite" draft fields, the ground station
    /// used for AOS/LOS, and the simulated clock's speed/offset.
    fn show_satellite_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Add satellite").strong());
        egui::ComboBox::from_id_salt("satellite-preset")
            .selected_text(match self.new_satellite_preset {
                Some(index) => PRESETS[index].name,
                None => "-- Manual --",
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.new_satellite_preset.is_none(), "-- Manual --")
                    .clicked()
                {
                    self.new_satellite_preset = None;
                }
                for (index, preset) in PRESETS.iter().enumerate() {
                    if ui
                        .selectable_label(self.new_satellite_preset == Some(index), preset.name)
                        .clicked()
                    {
                        self.new_satellite_preset = Some(index);
                        // Prefers a TLE that a catalog fetch saved to
                        // `tle_data.db` over the baked-in preset lines.
                        self.fill_draft_from_preset(index);
                    }
                }
            });

        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.new_satellite_name);
        });
        ui.horizontal(|ui| {
            ui.label("Line 1");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_tle_line1)
                    .desired_width(300.0)
                    .font(egui::TextStyle::Monospace),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Line 2");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_tle_line2)
                    .desired_width(300.0)
                    .font(egui::TextStyle::Monospace),
            );
        });
        if ui.button("Add satellite").clicked() {
            self.add_satellite();
        }

        // Search Satellite (Space-Track) section: Full panel in fn:show_catalog_search
        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Search Satellite (Space-Track)").strong());
        self.show_catalog_search(ui);

        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Ground station (AOS/LOS)").strong());
        ui.horizontal(|ui| {
            ui.label("Latitude");
            ui.add(
                egui::DragValue::new(&mut self.station_lat)
                    .speed(0.01)
                    .range(-90.0..=90.0),
            );
            ui.label("Longitude");
            ui.add(
                egui::DragValue::new(&mut self.station_lon)
                    .speed(0.01)
                    .range(-180.0..=180.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Elevation mask (deg)");
            ui.add(
                egui::DragValue::new(&mut self.elevation_mask_deg)
                    .speed(0.1)
                    .range(0.0..=90.0),
            );
        });
        ui.label(
            egui::RichText::new("Save SystemConfig.json to keep the satellite list and station")
                .small()
                .weak(),
        );

        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            if self.satellite_tracking {
                if ui.button("Stop Tracking").clicked() {
                    self.stop_satellite_tracking();
                }
            } else if ui.button("Generate Results").clicked() {
                self.start_satellite_tracking();
            }
        });
        if let Some(error) = &self.satellite_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }

        // Simulated time speed slider, with a reset button and a display of the current simulated time.
        ui.separator();
        ui.label("Simulated time speed (seconds per real second)");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.sim_time_speed, -600..=600));
            if ui.button("Reset").clicked() {
                self.sim_time_speed = 0;
                self.sim_time_offset_s = 0.0;
                self.sim_last_tick = None;
            }
        });
        if let Some(time) = self.simulated_time() {
            ui.label(format!(
                "Simulated time: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                time.year, time.month, time.day, time.hour, time.minute, time.second
            ));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Tracked satellites").strong());
        if self.tracked_satellites.is_empty() {
            ui.label("No satellites yet. Add one above.");
        }
        let mut to_remove = None;
        for (index, sat) in self.tracked_satellites.iter().enumerate() {
            let color = SATELLITE_COLORS[index % SATELLITE_COLORS.len()];
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color, "\u{25cf}");
                    ui.label(egui::RichText::new(&sat.name).strong());
                    if ui.small_button("Remove").clicked() {
                        to_remove = Some(index);
                    }
                });
                if let Some(error) = &sat.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                match (sat.position, sat.field) {
                    (Some(position), field) => {
                        egui::Grid::new(("satellite-result-grid", index))
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label("Latitude");
                                ui.label(format!("{:.3}", position.latitude));
                                ui.end_row();
                                ui.label("Longitude");
                                ui.label(format!("{:.3}", position.longitude));
                                ui.end_row();
                                ui.label("Altitude km");
                                ui.label(format!("{:.3}", position.altitude_km));
                                ui.end_row();
                                ui.label("Total Intensity");
                                match field.map(|f| f.total_intensity) {
                                    Some(value) => ui.label(format!("{value:.3}")),
                                    None => ui.label("--"),
                                };
                                ui.end_row();
                                ui.label("Elevation");
                                let elevation = elevation_deg(
                                    self.station_lat,
                                    self.station_lon,
                                    position.ecef_km,
                                );
                                ui.label(format!(
                                    "{elevation:.1} deg ({})",
                                    if sat.was_visible {
                                        "visible"
                                    } else {
                                        "below mask"
                                    }
                                ));
                                ui.end_row();
                            });
                    }
                    (None, _) => {
                        ui.label("No result yet - press \"Generate Results\".");
                    }
                }
            });
        }
        if let Some(index) = to_remove {
            self.remove_satellite(index);
        }
    }

    // Search the Space-Track catalog for satellites, filter, and show search results
    fn show_catalog_search(&mut self, ui: &mut egui::Ui) {
        let fetching = self.sat_search.fetch_task.is_some();

        ui.horizontal(|ui| {
            ui.label("Object type");
            let before = self.sat_search.object_type;
            egui::ComboBox::from_id_salt("sat-search-type")
                .selected_text(OBJECT_TYPE_CHOICES[self.sat_search.object_type].0)
                .show_ui(ui, |ui| {
                    for (index, (label, _)) in OBJECT_TYPE_CHOICES.iter().enumerate() {
                        ui.selectable_value(&mut self.sat_search.object_type, index, *label);
                    }
                });
            if self.sat_search.object_type != before {
                self.sat_search.page = 0;
            }

            let fetch = egui::Button::new(if fetching {
                "Fetching\u{2026}"
            } else {
                "Fetch data"
            });
            if ui
                .add_enabled(!fetching, fetch)
                .on_hover_text("Fetch every object of this type from Space-Track into tle_data.db")
                .clicked()
            {
                self.spawn_type_fetch();
            }
        });

        if let Some(error) = self.sat_search.error.clone() {
            ui.colored_label(Color32::LIGHT_RED, error);
        }

        if !self.sat_search.is_selected_type_fetched() {
            ui.add_space(4.0);
            ui.colored_label(Color32::YELLOW, "The data has not been updated");
            ui.label(
                egui::RichText::new("Click \"Fetch data\" to download this object type.")
                    .small()
                    .weak(),
            );
            return;
        }

        // Search filters over the fetched data
        egui::Grid::new("sat-search-filters")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Object name");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.object_name)
                        .hint_text("e.g. STARLINK"),
                );
                ui.end_row();

                ui.label("NORAD ID");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.norad_cat_id)
                        .hint_text("e.g. 25544"),
                );
                ui.end_row();

                ui.label("RCS size");
                egui::ComboBox::from_id_salt("sat-search-rcs")
                    .selected_text(RCS_OPTIONS[self.sat_search.rcs_size])
                    .show_ui(ui, |ui| {
                        for (index, label) in RCS_OPTIONS.iter().enumerate() {
                            ui.selectable_value(&mut self.sat_search.rcs_size, index, *label);
                        }
                    });
                ui.end_row();

                ui.label("Launch site");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.site)
                        .hint_text("e.g. Cape Canaveral"),
                );
                ui.end_row();

                ui.label("Country code");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.country_code)
                        .hint_text("e.g. USA"),
                );
                ui.end_row();

                ui.label("Launched before");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.launch_date)
                        .hint_text("yyyy-mm-dd"),
                );
                ui.end_row();

                ui.label("Decayed before");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.decay_date)
                        .hint_text("yyyy-mm-dd"),
                );
                ui.end_row();
            });
        if ui.button("Clear filters").clicked() {
            self.sat_search.object_name.clear();
            self.sat_search.norad_cat_id.clear();
            self.sat_search.rcs_size = 0;
            self.sat_search.site.clear();
            self.sat_search.country_code.clear();
            self.sat_search.launch_date.clear();
            self.sat_search.decay_date.clear();
        }

        // if input key changed (!= last_filter_key), reset the page and re-run the search
        let key = self.sat_search.filter_key();
        if key != self.sat_search.last_filter_key {
            self.sat_search.last_filter_key = key;
            self.sat_search.page = 0;
            self.run_catalog_search();
        }

        ui.add_space(4.0);
        let page = self.sat_search.page;
        let page_count = self.sat_search.page_count();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} result(s) \u{2014} page {} of {}",
                self.sat_search.total,
                page + 1,
                page_count
            ));
            if ui
                .add_enabled(page > 0, egui::Button::new("\u{2039} Prev"))
                .clicked()
            {
                self.sat_search.page -= 1;
                self.run_catalog_search();
            }
            if ui
                .add_enabled(page + 1 < page_count, egui::Button::new("Next \u{203a}"))
                .clicked()
            {
                self.sat_search.page += 1;
                self.run_catalog_search();
            }
        });

        let mut pending_add: Option<StoredTle> = None;
        egui::ScrollArea::vertical()
            .min_scrolled_height(400.0)
            .id_salt("sat-search-results")
            .show(ui, |ui| {
                for row in &self.sat_search.results {
                    let add = ui
                        .horizontal(|ui| {
                            let clicked = ui.small_button("Select").clicked();
                            ui.label(format!("{}  #{}", row.object_name, row.norad_cat_id));
                            clicked
                        })
                        .inner;
                    if add {
                        pending_add = Some(row.clone());
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "    {} \u{b7} RCS {} \u{b7} {} \u{b7} site {} \u{b7} launched {}{}",
                            dash_if_blank(&row.object_type),
                            dash_if_blank(&row.rcs_size),
                            dash_if_blank(&row.country_code),
                            dash_if_blank(&row.site),
                            dash_if_blank(&row.launch_date),
                            if row.decay_date.is_empty() {
                                String::new()
                            } else {
                                format!(" \u{b7} decayed {}", row.decay_date)
                            },
                        ))
                        .small()
                        .weak(),
                    );
                }
            });
        if let Some(row) = pending_add {
            if row.line1.trim().is_empty() || row.line2.trim().is_empty() {
                self.set_error(format!("{} has no TLE lines stored", row.object_name));
            } else {
                // Load the result to see the TLE lines before Add satellite
                self.new_satellite_preset = None;
                self.new_satellite_name = row.object_name.clone();
                self.new_tle_line1 = row.line1;
                self.new_tle_line2 = row.line2;
                self.set_status(format!(
                    "{} loaded into \"Add satellite\" \u{2014} press \"Add satellite\" to track it",
                    row.object_name
                ));
            }
        }
    }

    /// Right side of the IGRF Model tab: Magnetism Result
    fn show_model_result_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Geomagnetic field map");
        match &self.map_contours {
            Some(contours) if !contours.is_empty() => {
                let mut labels: Vec<(f64, [f64; 2])> = Vec::new();
                for segment in contours {
                    let already_labelled = labels
                        .iter()
                        .any(|(level, _)| (level - segment.level).abs() < f64::EPSILON);
                    if !already_labelled {
                        labels.push((
                            segment.level,
                            [
                                (segment.start[0] + segment.end[0]) / 2.0,
                                (segment.start[1] + segment.end[1]) / 2.0,
                            ],
                        ));
                    }
                }
                Plot::new(("model-map", self.map_view_generation))
                    .x_axis_label("Longitude")
                    .y_axis_label("Latitude")
                    .height(720.0)
                    .show(ui, |plot_ui| {
                        for segment in contours {
                            plot_ui.line(
                                Line::new(
                                    "Contour lines",
                                    PlotPoints::new(vec![segment.start, segment.end]),
                                )
                                .color(CONTOUR_LINE_COLOR),
                            );
                        }
                        for (level, point) in &labels {
                            plot_ui.text(
                                Text::new(
                                    format!("contour-label-{level}"),
                                    PlotPoint::new(point[0], point[1]),
                                    format!("{level:.0}"),
                                )
                                .color(CONTOUR_LINE_COLOR),
                            );
                        }
                        // Ground track + current position per tracked
                        // satellite. Colors match the list in the left
                        // panel, which doubles as the legend - a `.legend()`
                        // here would otherwise also pick up every contour
                        // segment above.
                        for (index, sat) in self.tracked_satellites.iter().enumerate() {
                            let color = SATELLITE_COLORS[index % SATELLITE_COLORS.len()];
                            // `.id(...)` overrides the id egui_plot would
                            // otherwise derive from `name` alone, which
                            // would collide if two entries share a name
                            // (e.g. the ISS preset added twice).
                            for (segment_index, segment) in sat.track_segments.iter().enumerate() {
                                plot_ui.line(
                                    Line::new(sat.name.clone(), PlotPoints::new(segment.clone()))
                                        .id(egui::Id::new((
                                            "satellite-track",
                                            index,
                                            segment_index,
                                        )))
                                        .color(color),
                                );
                            }
                            if let Some(position) = sat.position {
                                plot_ui.points(
                                    Points::new(
                                        sat.name.clone(),
                                        vec![[position.longitude, position.latitude]],
                                    )
                                    .id(egui::Id::new(("satellite-point", index)))
                                    .radius(5.0)
                                    .color(color),
                                );
                            }
                        }
                    });
            }
            Some(_) => {
                ui.label("No contour lines at this level step for the loaded grid.");
            }
            None => {
                ui.label("No geomagnetic field map generated yet.");
            }
        }

        if self
            .tracked_satellites
            .iter()
            .any(|sat| !sat.field_track.is_empty())
        {
            ui.add_space(8.0);
            ui.heading("Satellite field intensity vs time");
            Plot::new("satellite-field-vs-time")
                .x_axis_label("Minutes from simulated time")
                .y_axis_label("Total intensity (nT)")
                .height(280.0)
                .legend(Legend::default())
                .show(ui, |plot_ui| {
                    for (index, sat) in self.tracked_satellites.iter().enumerate() {
                        if sat.field_track.is_empty() {
                            continue;
                        }
                        let color = SATELLITE_COLORS[index % SATELLITE_COLORS.len()];
                        plot_ui.line(
                            Line::new(sat.name.clone(), PlotPoints::new(sat.field_track.clone()))
                                .id(egui::Id::new(("satellite-field", index)))
                                .color(color),
                        );
                    }
                });
        }
    }

    /// One X/Y/Z card: live readout on top, PID gains below. Always visible so
    /// nothing that can stop an axis hides behind navigation.
    fn axis_column(&mut self, ui: &mut egui::Ui, axis: usize) {
        let label = AXES[axis];
        let target = self.slew.target();
        let mut command = None;
        let mut pause = false;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width().max(0.0));
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.spacing_mut().interact_size.y = 16.0;
            ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
            for font in ui.style_mut().text_styles.values_mut() {
                font.size *= 0.8;
            }
            let running = self.pid_running[axis];
            ui.horizontal(|ui| {
                status_pill(ui, &format!("Axis {label}"), LinkState::from_open(running));
                if ui
                    .small_button(if running { "Pause" } else { "Start" })
                    .clicked()
                {
                    self.pid_running[axis] = !running;
                    if !self.pid_running[axis] {
                        pause = true;
                    }
                }
                if ui.small_button("Reset").clicked() {
                    self.reset_axis(axis);
                }
            });

            let error = self.pid_settings[axis].setpoint - self.filtered[axis];
            let error_percent = [
                self.processed.error_per_x,
                self.processed.error_per_y,
                self.processed.error_per_z,
            ][axis];

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:+.3}", self.filtered[axis]))
                        .monospace()
                        .size(16.0)
                        .strong(),
                );
                ui.label(egui::RichText::new("nT").small());
                ui.separator();
                ui.label(format!("set {:+.3}", self.pid_settings[axis].setpoint));
                if self.pid_settings[axis].setpoint == 0.0 {
                    ui.label(format!("err {error:+.3}"));
                } else {
                    ui.colored_label(
                        error_color(error_percent),
                        format!("err {error:+.3} ({error_percent:.2}%)"),
                    );
                }
            });
            ui.label(
                egui::RichText::new(format!(
                    "raw {:+.3}   cal {:+.3}",
                    self.raw[axis], self.calibrated[axis]
                ))
                .weak(),
            );

            let fraction = output_fraction(
                self.outputs[axis],
                self.pid_settings[axis].min_output,
                self.pid_settings[axis].max_output,
            );
            // At the limit the loop has no authority left, so the reading looks
            // steady for the wrong reason. Say so instead of just filling the bar.
            let saturated = running
                && (self.outputs[axis] >= self.pid_settings[axis].max_output
                    || self.outputs[axis] <= self.pid_settings[axis].min_output);
            let mut bar = egui::ProgressBar::new(fraction as f32).text(if saturated {
                format!("output {:+.3}  SATURATED", self.outputs[axis])
            } else {
                format!("output {:+.3}", self.outputs[axis])
            });
            if saturated {
                bar = bar.fill(STOP_RED);
            }
            ui.add(bar);

            // Tuning laid out in three columns so all three axis cards still
            // fit a 1024x600 screen without a dropdown.
            ui.separator();
            let ceiling = FIRMWARE_MAX_OUTPUT[axis];
            ui.columns(3, |cols| {
                egui::Grid::new(format!("pid-grid-{axis}"))
                    .num_columns(2)
                    .striped(true)
                    .show(&mut cols[0], |ui| {
                        for (name, value) in [
                            ("Kp", &mut self.pid_settings[axis].kp),
                            ("Ki", &mut self.pid_settings[axis].ki),
                            ("Kd", &mut self.pid_settings[axis].kd),
                        ] {
                            ui.label(name);
                            ui.add(egui::DragValue::new(value).speed(0.1));
                            ui.end_row();
                        }
                    });

                egui::Grid::new(format!("out-grid-{axis}"))
                    .num_columns(2)
                    .striped(true)
                    .show(&mut cols[1], |ui| {
                        ui.label("Setpoint");
                        let mut commanded = target[axis];
                        if ui
                            .add(egui::DragValue::new(&mut commanded).speed(1.0))
                            .changed()
                        {
                            let mut field = target;
                            field[axis] = commanded;
                            command = Some(field);
                        }
                        ui.end_row();
                        ui.label("Min out");
                        ui.add(
                            egui::DragValue::new(&mut self.pid_settings[axis].min_output)
                                .speed(1.0)
                                .range(-ceiling..=0.0),
                        );
                        ui.end_row();
                        ui.label("Max out")
                            .on_hover_text(format!("firmware ceiling {ceiling:.0}"));
                        ui.add(
                            egui::DragValue::new(&mut self.pid_settings[axis].max_output)
                                .speed(1.0)
                                .range(0.0..=ceiling),
                        );
                        ui.end_row();
                    });

                egui::Grid::new(format!("filter-grid-{axis}"))
                    .num_columns(2)
                    .striped(true)
                    .show(&mut cols[2], |ui| {
                        for (name, value, speed) in [
                            ("Q proc", &mut self.filter_settings[axis].q, 0.05),
                            ("R meas", &mut self.filter_settings[axis].r, 1.0),
                            ("Spike", &mut self.filter_settings[axis].spike_nt, 50.0),
                        ] {
                            ui.label(name);
                            ui.add(egui::DragValue::new(value).speed(speed).range(1e-6..=1e9));
                            ui.end_row();
                        }
                    });
            });
        });
        if pause {
            // Pausing one axis stops that axis' PID, but the controller holds
            // whatever it was last sent for all three. Push a packet now with
            // this axis at zero instead of waiting for the next tick.
            self.pids[axis].hold();
            self.outputs[axis] = 0.0;
            self.write_outputs();
        }
        if let Some(field) = command {
            self.setpoint_source = SetpointSource::Manual;
            self.command_setpoint(field);
        }
    }

    fn show_control_columns(&mut self, ui: &mut egui::Ui) {
        if !fits_columns(ui, 3) {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    self.show_magson_strip(ui);
                    ui.separator();
                    for axis in 0..3 {
                        self.axis_column(ui, axis);
                    }
                    ui.separator();
                    self.show_plots_header(ui);
                    for axis in 0..3 {
                        self.axis_sensor_plot(ui, axis, 160.0);
                    }
                    ui.separator();
                    self.show_cage(ui, 240.0);
                    self.magnitude_plot(ui, 160.0);
                    self.magson_plot(ui, 160.0);
                });
            return;
        }

        let budget = ui.available_height().clamp(320.0, 900.0);
        let plot_h = ((budget - 22.0 - 3.0 * 16.0) / 3.0).clamp(70.0, 190.0);
        let cage_h = (budget - 20.0 - 2.0 * (plot_h + 16.0)).clamp(110.0, 240.0);

        ui.columns(3, |columns| {
            self.show_magson_strip(&mut columns[0]);
            columns[0].separator();
            for axis in 0..3 {
                self.axis_column(&mut columns[0], axis);
                columns[0].add_space(2.0);
            }

            self.show_plots_header(&mut columns[1]);
            for axis in 0..3 {
                self.axis_sensor_plot(&mut columns[1], axis, plot_h);
            }

            self.show_cage(&mut columns[2], cage_h);
            self.magnitude_plot(&mut columns[2], plot_h);
            self.magson_plot(&mut columns[2], plot_h);
        });
    }

    fn show_magson_strip(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Magson").strong().small());
            for (label, value) in [
                ("X", self.magson[0]),
                ("Y", self.magson[1]),
                ("Z", self.magson[2]),
                ("|B|", self.magson_total),
            ] {
                ui.label(
                    egui::RichText::new(format!("{label} {value:+.3}"))
                        .monospace()
                        .small(),
                );
            }
        });
    }

    fn show_cage(&mut self, ui: &mut egui::Ui, max_size: f32) {
        // The view wants signed drive normalised against each axis' own limit,
        // not raw controller units.
        let drive = std::array::from_fn(|axis| {
            let settings = &self.pid_settings[axis];
            let span = settings.min_output.abs().max(settings.max_output.abs());
            if span > 0.0 {
                (self.outputs[axis] / span).clamp(-1.0, 1.0)
            } else {
                0.0
            }
        });
        egui::CollapsingHeader::new("Coil cage")
            .default_open(true)
            .show(ui, |ui| cage::show(ui, &mut self.cage, drive, max_size));
    }

    fn show_plots_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Live plots").strong().small());
            ui.checkbox(&mut self.follow_plots, "Follow live");
        });
    }

    fn axis_sensor_plot(&self, ui: &mut egui::Ui, axis: usize, height: f32) {
        show_plot(
            ui,
            &format!("sensor-plot-{axis}"),
            &format!("{} nT: setpoint vs measured", AXES[axis]),
            &[
                (
                    "Setpoint",
                    &self.history.sensor_setpoint[axis],
                    Color32::LIGHT_RED,
                ),
                (
                    "Measured",
                    &self.history.sensor_measured[axis],
                    Color32::LIGHT_BLUE,
                ),
            ],
            self.follow_plots,
            height,
        );
    }

    fn magnitude_plot(&self, ui: &mut egui::Ui, height: f32) {
        show_plot(
            ui,
            "sensor-magnitude-plot",
            "|B| nT: setpoint vs measured",
            &[
                (
                    "Setpoint",
                    &self.history.sensor_magnitude_setpoint,
                    Color32::LIGHT_RED,
                ),
                (
                    "Measured",
                    &self.history.sensor_magnitude_measured,
                    Color32::LIGHT_BLUE,
                ),
            ],
            self.follow_plots,
            height,
        );
    }

    fn magson_plot(&self, ui: &mut egui::Ui, height: f32) {
        show_plot(
            ui,
            "magson-plot",
            "Magson X/Y/Z/total (nT)",
            &[
                ("X", &self.history.magson[0], Color32::LIGHT_RED),
                ("Y", &self.history.magson[1], Color32::LIGHT_GREEN),
                ("Z", &self.history.magson[2], Color32::LIGHT_BLUE),
                ("Total", &self.history.magson[3], Color32::YELLOW),
            ],
            self.follow_plots,
            height,
        );
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

impl eframe::App for IgrfApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_io();
        self.run_pid();
        self.tick_satellite_tracking();
        if ctx.input(|input| input.key_pressed(egui::Key::F11)) {
            self.fullscreen = !self.fullscreen;
            if self.fullscreen {
                // Drop decorations and fullscreen onto the named screen, so the
                // window covers the whole panel with no title bar - matching
                // how `Display.Mode = Fullscreen` builds the window at startup.
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::SetMonitor(
                    self.config.display.fullscreen_monitor,
                ));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
            }
        }
        ctx.request_repaint_after(UI_INTERVAL);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("tab-strip").show(ui, |ui| {
            self.show_tab_strip(ui);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Status: {}", self.status));
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::LIGHT_RED, format!("Error: {error}"));
                    if ui.small_button("Dismiss").clicked() {
                        self.error = None;
                    }
                }
            });
        });
        match self.active_tab {
            AppTab::Control => {
                egui::Panel::top("top-bar").show(ui, |ui| {
                    ui.add_space(2.0);
                    self.show_top_bar(ui);
                    ui.add_space(2.0);
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    self.show_control_columns(ui);
                });
            }
            AppTab::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            // The left column carries only short forms, so give
                            // it less width than the right, which holds the
                            // wider setpoint / calibration / logging panels.
                            ui.horizontal_top(|ui| {
                                let gap = ui.spacing().item_spacing.x;
                                let total = ui.available_width();
                                let left = ((total - gap) * 0.36).max(180.0);
                                let right = (total - gap - left).max(220.0);

                                ui.allocate_ui_with_layout(
                                    egui::vec2(left, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(left);
                                        egui::CollapsingHeader::new("Connections")
                                            .default_open(true)
                                            .show(ui, |ui| self.show_connection_panel(ui));
                                        egui::CollapsingHeader::new("LAN static IP")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_lan_panel(ui));
                                        egui::CollapsingHeader::new("Display")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_display_panel(ui));
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    egui::vec2(right, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(right);
                                        egui::CollapsingHeader::new("Setpoint command")
                                            .default_open(true)
                                            .show(ui, |ui| self.show_setpoint_panel(ui));
                                        egui::CollapsingHeader::new("Sensor calibration")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_calibration_panel(ui));
                                        egui::CollapsingHeader::new("Config / logging")
                                            .default_open(true)
                                            .show(ui, |ui| self.show_config_panel(ui));
                                        egui::CollapsingHeader::new("Saved log files")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_log_files_panel(ui));
                                    },
                                );
                            });
                        });
                });
            }
            AppTab::Model => {
                egui::Panel::left("model-setup")
                    .resizable(true)
                    .default_size(300.0)
                    .size_range(240.0..=460.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui::CollapsingHeader::new("IGRF Model")
                                .default_open(true)
                                .show(ui, |ui| self.show_map_panel(ui));
                            egui::CollapsingHeader::new("Time")
                                .default_open(true)
                                .show(ui, |ui| self.show_time_panel(ui));
                            egui::CollapsingHeader::new("Manual Magnetism Calculator")
                                .default_open(true)
                                .show(ui, |ui| self.show_manual_panel(ui));
                            egui::CollapsingHeader::new("Satellite Position")
                                .default_open(true)
                                .show(ui, |ui| self.show_satellite_panel(ui));
                        });
                    });
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| self.show_model_result_panel(ui));
                });
            }
        }
    }
}

/// One row in either file list of the Config / logging panel.
struct FileRow {
    name: String,
    is_dir: bool,
    size_bytes: u64,
}

/// Clear a remembered file selection once that file is no longer a plain file in
/// the freshly scanned list (deleted, renamed, or navigated away from).
fn prune_selection(selected: &mut Option<String>, rows: &[FileRow]) {
    if let Some(name) = selected {
        if !rows.iter().any(|row| !row.is_dir && &row.name == name) {
            *selected = None;
        }
    }
}

/// Byte count as a short human string (`1.4 MB`), for the log file list.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
