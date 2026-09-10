use crate::control_helpers::pid_from_settings;
use crate::satellite_ui::TrackedSat;
use crate::{IgrfApp, AXES, CONFIG_PATH};
use igrf_core::{AppConfig, SlewLimiter, FIRMWARE_MAX_OUTPUT};

impl IgrfApp {
    pub(crate) fn save_config(&mut self) {
        if self.sensor_baud == 0 {
            self.set_error("Cannot save: sensor baud must be greater than zero");
            return;
        }
        if self.controller_baud == 0 {
            self.set_error("Cannot save: controller baud must be greater than zero");
            return;
        }
        if self.magson_port == 0 {
            self.set_error("Cannot save: Magson port must be greater than zero");
            return;
        }
        self.config.pid_x = self.pid_settings[0].clone();
        self.config.pid_y = self.pid_settings[1].clone();
        self.config.pid_z = self.pid_settings[2].clone();
        self.config.filter_x = self.filter_settings[0].clone();
        self.config.filter_y = self.filter_settings[1].clone();
        self.config.filter_z = self.filter_settings[2].clone();
        self.config.sensor_port = self.sensor_port.clone();
        self.config.sensor_baud = self.sensor_baud;
        self.config.controller_port = self.controller_port.clone();
        self.config.controller_baud = self.controller_baud;
        self.config.sensor2_ip = self.magson_ip.clone();
        self.config.sensor2_port = i32::from(self.magson_port);
        self.config.calibration = self.calibration.clone();
        self.config.setpoint_profile_path = self.profile_path.clone();
        self.config.setpoint_source_bind_address = self.setpoint_bind_address.clone();
        self.config.setpoint_source_port = i32::from(self.setpoint_port);
        self.config.satellites = self
            .tracked_satellites
            .iter()
            .map(TrackedSat::to_entry)
            .collect();
        self.config.station_latitude = self.station_lat;
        self.config.station_longitude = self.station_lon;
        self.config.elevation_mask_deg = self.elevation_mask_deg;
        let clamped = self.config.sanitize();
        // sanitize() may have pulled these back inside range; follow it for
        // the same reason the PID panel does below.
        self.station_lat = self.config.station_latitude;
        self.station_lon = self.config.station_longitude;
        self.elevation_mask_deg = self.config.elevation_mask_deg;
        // Saving writes back whatever sanitize settled on, so the panel has to
        // follow or the UI would keep showing a limit the file no longer holds.
        self.pid_settings = [
            self.config.pid_x.clone(),
            self.config.pid_y.clone(),
            self.config.pid_z.clone(),
        ];
        match self.config.save(CONFIG_PATH) {
            Ok(()) if clamped.iter().any(|was| *was) => self.set_error(format!(
                "Saved {CONFIG_PATH}, but output limits on {} were pulled inside the \
                 firmware ceiling ({:.0}/{:.0}/{:.0})",
                AXES.into_iter()
                    .zip(clamped)
                    .filter(|(_, was)| *was)
                    .map(|(name, _)| name.to_string())
                    .collect::<Vec<_>>()
                    .join("/"),
                FIRMWARE_MAX_OUTPUT[0],
                FIRMWARE_MAX_OUTPUT[1],
                FIRMWARE_MAX_OUTPUT[2],
            )),
            Ok(()) => self.set_status(format!("Saved {CONFIG_PATH}")),
            Err(error) => self.set_error(format!("Cannot save {CONFIG_PATH}: {error}")),
        }
    }

    pub(crate) fn load_config(&mut self) {
        let (config, problem) = AppConfig::load(CONFIG_PATH);
        self.sensor_port = config.sensor_port.clone();
        self.sensor_baud = config.sensor_baud;
        self.controller_port = config.controller_port.clone();
        self.controller_baud = config.controller_baud;
        self.magson_ip = config.sensor2_ip.clone();
        self.magson_port = config.sensor2_port as u16;
        self.pid_settings = [
            config.pid_x.clone(),
            config.pid_y.clone(),
            config.pid_z.clone(),
        ];
        self.pids = self.pid_settings.clone().map(pid_from_settings);
        self.filter_settings = [
            config.filter_x.clone(),
            config.filter_y.clone(),
            config.filter_z.clone(),
        ];
        self.calibration = config.calibration.clone();
        self.sensor_service.calibration = config.calibration.clone();

        // Trackers aren't serializable, so a loaded satellite list is
        // rebuilt from its TLE text rather than restored directly.
        self.tracked_satellites = config
            .satellites
            .iter()
            .map(TrackedSat::from_entry)
            .collect();
        self.station_lat = config.station_latitude;
        self.station_lon = config.station_longitude;
        self.elevation_mask_deg = config.elevation_mask_deg;

        self.profile_path = config.setpoint_profile_path.clone();
        self.slew_rate = config.setpoint_slew_nt_per_second;
        self.setpoint_bind_address = config.setpoint_source_bind_address.clone();
        if config.setpoint_source_port > 0 {
            self.setpoint_port = config.setpoint_source_port as u16;
        }
        // A loaded config brings its own setpoints; start the ramp there
        // rather than sweeping from wherever the last one left off.
        self.slew = SlewLimiter::new(
            config.setpoint_slew_nt_per_second,
            std::array::from_fn(|axis| self.pid_settings[axis].setpoint),
        );
        self.config = config;
        match problem {
            Some(message) => self.set_error(message),
            None => match self
                .calibration_warning()
                .or_else(|| self.authority_warning())
            {
                Some(warning) => self.set_error(format!("Loaded {CONFIG_PATH}, but {warning}")),
                None => self.set_status(format!("Loaded {CONFIG_PATH}")),
            },
        }
    }
}
