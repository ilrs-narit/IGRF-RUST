use crate::cage;
use crate::control_helpers::{pid_from_settings, stable_first};
use crate::history::PlotHistory;
use crate::netcfg;
use crate::satellite_ui::{SatSearchState, TrackedSat};
use crate::{AppTab, IgrfApp, SetpointSource};
use igrf_core::satellite::PRESETS;
use igrf_core::{
    AppConfig, CalculationService, DisplayMode, ProcessedData, SensorService, SlewLimiter,
};
use igrf_io::{ControllerReplyCounter, MagsonTcpClient, SerialPortManager, SetpointServer};
use std::time::Instant;

impl IgrfApp {
    pub(crate) fn new(
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
}
