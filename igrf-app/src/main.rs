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

use control_helpers::{pid_from_settings, stable_first};
use history::PlotHistory;
use satellite_ui::{SatSearchState, TrackedSat};

use eframe::egui::{self, Color32};
use igrf_core::geomagnetism::GeomagnetismResult;
use igrf_core::satellite::PRESETS;
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
use std::sync::mpsc::{self, Receiver};
use std::thread;
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
