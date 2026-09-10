use crate::control_helpers::{apply_pid_settings, loop_fault, validate_pid_settings, LoopFault};
use crate::{IgrfApp, AXES};
use igrf_core::ProcessedData;
use igrf_io::write_controller_packet;
use std::time::{Duration, Instant};

const PID_INTERVAL: Duration = Duration::from_millis(100);

impl IgrfApp {
    pub(crate) fn run_pid(&mut self) {
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
    pub(crate) fn write_outputs(&mut self) {
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

    pub(crate) fn reset_controller_link_stats(&mut self) {
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
    pub(crate) fn poll_controller_replies(&mut self) {
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

    pub(crate) fn reset_axis(&mut self, axis: usize) {
        self.pids[axis].reset();
        match axis {
            0 => self.calculation.reset_filter_x(),
            1 => self.calculation.reset_filter_y(),
            _ => self.calculation.reset_filter_z(),
        }
        self.outputs[axis] = 0.0;
        self.set_status(format!("Reset axis {} PID/filter", AXES[axis]));
    }

    pub(crate) fn master_reset(&mut self) {
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

    pub(crate) fn stop_all(&mut self) {
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
    pub(crate) fn watchdog_pause(&mut self) {
        self.pid_running = [false; 3];
        for pid in &mut self.pids {
            pid.hold();
        }
        if let Err(error) = self.zero_outputs() {
            self.set_error(format!("Watchdog pause: controller write failed: {error}"));
        }
    }
}
