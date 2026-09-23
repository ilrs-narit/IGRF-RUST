use crate::IgrfApp;
use igrf_core::PidSettings;
use igrf_io::CsvLogger;
use std::time::Duration;

/// Column-for-column the header the C# build wrote, so existing analysis
/// scripts keep working against logs from either implementation.
/// The C# columns, in the C# order, plus the four this build adds at the end:
/// the commanded setpoint before the slew limiter and the real tick interval.
/// Appending keeps every existing analysis script working.
pub(crate) const LOG_HEADER: &str = "Timestamp,MagX,MagY,MagZ,MagTotal,SetX,SetY,SetZ,SetTotal,ErrX,ErrY,ErrZ,OutX,OutY,OutZ,KpX,KiX,KdX,KpY,KiY,KdY,KpZ,KiZ,KdZ,Mag2X,Mag2Y,Mag2Z,Mag2Total,CmdX,CmdY,CmdZ,TickMs";

impl IgrfApp {
    /// Formats one CSV row. Free-standing so a test can check it against
    /// [`LOG_HEADER`] without a running app.
    ///
    /// The first 28 columns match the C# row exactly: the filtered field as
    /// `Mag*`, the unsigned error from `ProcessedData`, F2 everywhere except
    /// the F3 gains. `Cmd*` and `TickMs` are appended by this build.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn snapshot_row(
        timestamp: &str,
        filtered: [f64; 3],
        setpoints: [f64; 3],
        errors: [f64; 3],
        gains: &[PidSettings; 3],
        outputs: [f64; 3],
        magson: [f64; 3],
        commanded: [f64; 3],
        tick: Duration,
    ) -> String {
        let magnitude =
            |values: [f64; 3]| values.iter().map(|value| value * value).sum::<f64>().sqrt();
        format!(
            "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.1}",
            timestamp,
            filtered[0],
            filtered[1],
            filtered[2],
            magnitude(filtered),
            setpoints[0],
            setpoints[1],
            setpoints[2],
            magnitude(setpoints),
            errors[0],
            errors[1],
            errors[2],
            outputs[0],
            outputs[1],
            outputs[2],
            gains[0].kp,
            gains[0].ki,
            gains[0].kd,
            gains[1].kp,
            gains[1].ki,
            gains[1].kd,
            gains[2].kp,
            gains[2].ki,
            gains[2].kd,
            magson[0],
            magson[1],
            magson[2],
            magnitude(magson),
            commanded[0],
            commanded[1],
            commanded[2],
            tick.as_secs_f64() * 1000.0,
        )
    }

    pub(crate) fn log_snapshot(&mut self, tick: Duration) {
        if self.logger.is_none() {
            return;
        }
        let line = Self::snapshot_row(
            &chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            self.filtered,
            std::array::from_fn(|axis| self.pid_settings[axis].setpoint),
            [
                self.processed.error_x,
                self.processed.error_y,
                self.processed.error_z,
            ],
            &self.pid_settings,
            self.outputs,
            self.magson,
            // Where the ramp is headed, as opposed to the setpoints, which are
            // where it has reached this tick. Without both, a log cannot tell a
            // slow ramp from a small command.
            self.slew.target(),
            tick,
        );
        let result = self.logger.as_mut().map(|logger| logger.write_line(&line));
        if let Some(Err(error)) = result {
            self.handle_logger_failure(error);
        }
    }

    /// A failed CSV write or flush is an experiment fault, not a logging
    /// inconvenience: the run takes the normal stop path and the fault is
    /// latched. The latch is the single alert - the bottom bar shows it until
    /// the operator acknowledges, and the control loop refuses to auto-resume
    /// while it stands.
    pub(crate) fn handle_logger_failure(&mut self, error: std::io::Error) {
        // Fire-once: log_snapshot early-returns while the logger is None, so
        // a latched fault is never overwritten by a later tick.
        self.logger = None;
        self.logger_fault = Some(format!(
            "CSV write failed: {error}. Experiment stopped, outputs zeroed; auto-resume disabled until acknowledged."
        ));
        self.stop_all();
    }

    /// The only way the latch clears: an explicit operator acknowledgement.
    pub(crate) fn acknowledge_logger_fault(&mut self) {
        self.logger_fault = None;
        self.set_status("CSV fault acknowledged");
    }

    pub(crate) fn start_logging(&mut self) {
        if self.log_path.trim().is_empty() {
            self.set_error("Log path is empty");
            return;
        }
        match CsvLogger::open(self.log_path.trim(), LOG_HEADER) {
            Ok(logger) => {
                self.set_status(format!("Logging to {}", logger.path().display()));
                self.logger = Some(logger);
            }
            Err(error) => self.set_error(format!("Cannot start CSV logging: {error}")),
        }
    }

    pub(crate) fn stop_logging(&mut self) {
        self.logger = None;
        self.set_status("Logging stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use igrf_core::AppConfig;
    use std::io::{self, Write};
    use std::time::Instant;

    // Known limitation: these tests run headless, so controller_manager is
    // closed and zero_outputs() returns Ok right after setting
    // outputs = [0.0; 3]. Wire-level proof that the zero packet reaches the
    // coils needs hardware.

    fn app() -> IgrfApp {
        IgrfApp::new(&egui::Context::default(), AppConfig::default(), None)
    }

    /// fail_write = true fails the row write (BrokenPipe, e.g. disk full);
    /// false lets the write through and fails the flush instead.
    struct FailSink {
        fail_write: bool,
    }

    impl Write for FailSink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "disk full"))
            } else {
                Ok(b.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_write {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::Other, "flush failed"))
            }
        }
    }

    fn failing_logger(name: &str, fail_write: bool) -> CsvLogger {
        let series = std::env::temp_dir()
            .join(format!("igrf-log-{}-{name}", std::process::id()))
            .join("sensor_log.csv");
        CsvLogger::with_writer(series, LOG_HEADER, Box::new(FailSink { fail_write }))
    }

    fn assert_run_stopped_and_latched(app: &IgrfApp) {
        assert!(app.logger.is_none(), "a failed logger is dropped");
        assert!(app.logger_fault.is_some(), "the fault is latched");
        assert_eq!(app.pid_running, [false; 3], "every axis stops");
        assert_eq!(app.outputs, [0.0; 3], "outputs are zeroed");
        assert!(!app.resume_pending, "resume intent is cancelled");
        assert_eq!(
            app.paused_by_watchdog, [false; 3],
            "no resume state survives"
        );
    }

    #[test]
    fn a_csv_write_failure_stops_the_run_and_latches_the_fault() {
        let mut app = app();
        app.pid_running = [true; 3];
        app.logger = Some(failing_logger("write-failure", true));

        app.log_snapshot(Duration::from_millis(100));

        assert_run_stopped_and_latched(&app);
    }

    #[test]
    fn a_csv_flush_failure_stops_the_run_and_latches_the_fault() {
        let mut app = app();
        app.pid_running = [true; 3];
        app.logger = Some(failing_logger("flush-failure", false));

        app.log_snapshot(Duration::from_millis(100));

        assert_run_stopped_and_latched(&app);
    }

    #[test]
    fn a_csv_failure_on_a_watchdog_tick_still_clears_the_resume_state() {
        let mut app = app();
        app.pid_running = [true; 3];
        app.logger = Some(failing_logger("watchdog-tick", true));
        // The controller is closed, so the watchdog fires on this tick and
        // saves resume state right before the log write fails.
        app.last_pid_tick = Instant::now() - Duration::from_millis(250);

        app.run_pid();

        assert_eq!(app.pid_running, [false; 3]);
        assert_eq!(
            app.paused_by_watchdog, [false; 3],
            "the handler's stop_all wins over the state the watchdog just saved"
        );
        assert!(app.logger_fault.is_some(), "the fault is latched");
    }

    #[test]
    fn a_latched_csv_fault_cancels_auto_resume_until_acknowledged() {
        let mut app = app();
        app.logger = Some(failing_logger("latched-resume", true));
        app.log_snapshot(Duration::from_millis(100));
        assert!(app.logger_fault.is_some());

        // Resume intent recorded by a reconnect while the fault stands.
        app.resume_pending = true;
        app.paused_by_watchdog = [true; 3];
        app.last_pid_tick = Instant::now() - Duration::from_millis(250);

        app.run_pid();

        assert_eq!(app.pid_running, [false; 3], "the run stays stopped");
        assert!(!app.resume_pending, "the stale resume intent is cancelled");
        assert!(app.logger_fault.is_some(), "the latch survives the tick");

        app.acknowledge_logger_fault();
        assert!(app.logger_fault.is_none());
    }

    #[test]
    fn the_csv_fault_latch_survives_everything_but_acknowledge() {
        let mut app = app();
        app.logger = Some(failing_logger("persistence", true));
        app.log_snapshot(Duration::from_millis(100));
        assert!(app.logger_fault.is_some());

        app.set_status("x");
        assert!(app.logger_fault.is_some());
        app.stop_all();
        assert!(app.logger_fault.is_some());
        app.error = None;
        assert!(app.logger_fault.is_some());

        app.acknowledge_logger_fault();
        assert!(app.logger_fault.is_none());
    }

    #[test]
    fn starting_logging_while_latched_does_not_clear_the_fault() {
        let mut app = app();
        app.logger = Some(failing_logger("start-while-latched", true));
        app.log_snapshot(Duration::from_millis(100));
        assert!(app.logger_fault.is_some());

        let dir = std::env::temp_dir().join(format!(
            "igrf-log-{}-start-while-latched-real",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        app.log_path = dir.join("sensor_log.csv").to_string_lossy().into_owned();
        app.start_logging();

        assert!(app.logger.is_some(), "logging starts normally");
        assert!(app.logger_fault.is_some(), "the latch is not cleared");
        app.logger = None;
        let _ = std::fs::remove_dir_all(dir);
    }
}
