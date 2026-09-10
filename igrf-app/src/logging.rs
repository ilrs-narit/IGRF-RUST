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
            self.logger = None;
            self.set_error(format!("CSV write failed: {error}"));
        }
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
