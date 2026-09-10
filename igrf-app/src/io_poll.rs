use crate::IgrfApp;
use igrf_core::NOMINAL_TICK_SECONDS;
use igrf_io::MagsonSample;
use std::time::{Duration, Instant, SystemTime};

const HANDSHAKE: [u8; 6] = [0x2A, 0x30, 0x30, 0x57, 0x45, 0x0D];

impl IgrfApp {
    pub(crate) fn poll_io(&mut self) {
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

    pub(crate) fn handle_sensor_packet(&mut self, packet: &[u8]) {
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
    pub(crate) fn clear_magson(&mut self) {
        self.magson = [0.0; 3];
        self.magson_total = 0.0;
    }

    pub(crate) fn handle_magson_sample(&mut self, sample: MagsonSample) {
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
    pub(crate) fn sensor_change_age(&self) -> Option<Duration> {
        Some(self.last_sensor_change?.elapsed())
    }

    pub(crate) fn sensor_age(&self) -> Option<Duration> {
        let monotonic = self.last_sensor_packet?.elapsed();
        let wall = self
            .last_sensor_packet_wall
            .and_then(|at| at.elapsed().ok())
            .unwrap_or(Duration::ZERO);
        Some(monotonic.max(wall))
    }
}
