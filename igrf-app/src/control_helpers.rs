use igrf_core::{PidController, PidSettings};
use std::time::Duration;

/// A running loop is stopped once the newest sensor packet is older than this.
pub const SENSOR_TIMEOUT: Duration = Duration::from_millis(1000);
/// How long every raw count may sit unchanged before the sensor counts as dead.
///
/// The staleness watchdog only sees missing packets. A sensor that keeps
/// sending the same reading is worse: the loop believes it, the error stays
/// constant, the integrator winds to its clamp and the coils drive hard while
/// the real field walks away unmeasured.
///
/// Well above SENSOR_TIMEOUT because this is a claim about physics rather than
/// about the link. One HMR2300 count is 6.667 nT and its noise floor is larger
/// than that, so all three axes holding identical counts for seconds is a
/// frozen sensor, not a quiet cage.
pub const SENSOR_FROZEN_TIMEOUT: Duration = Duration::from_secs(5);

/// An open port that stopped delivering packets leaves the last reading in
/// place, and the PID would keep integrating against that frozen value until the
/// output saturates. Never having received a packet counts as stale too.
/// Whether a bind address keeps the setpoint listener on this machine.
///
/// Resolved rather than string-matched: `localhost` is loopback, `0.0.0.0` is
/// not, and neither is obvious from the text. An address that will not resolve
/// is reported as exposed, because the warning has to be the safe default.
pub fn is_loopback(address: &str) -> bool {
    use std::net::ToSocketAddrs;
    (address, 0_u16)
        .to_socket_addrs()
        .map(|mut resolved| resolved.all(|socket| socket.ip().is_loopback()))
        .unwrap_or(false)
}

/// Unlike [`sensor_is_stale`], `None` is not a fault: it only means no second
/// packet has arrived yet, which staleness already covers.
pub fn sensor_is_frozen(age: Option<Duration>) -> bool {
    age.is_some_and(|age| age > SENSOR_FROZEN_TIMEOUT)
}

/// Why the loop must not be driving the coils, if it must not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopFault {
    ControllerDown,
    SensorFrozen,
    SensorStale,
}

/// The one gate every path uses, so pausing and resuming cannot disagree about
/// what counts as healthy.
///
/// Ordered by what the operator has to deal with first. A controller that is
/// gone makes the sensor's state irrelevant - and is the worse fault, because
/// the firmware has no receive timeout and holds its last command until the
/// port reopens. A frozen sensor outranks a silent one only because it is the
/// more specific diagnosis of the two.
pub fn loop_fault(
    controller_open: bool,
    sensor_age: Option<Duration>,
    sensor_change_age: Option<Duration>,
) -> Option<LoopFault> {
    if !controller_open {
        return Some(LoopFault::ControllerDown);
    }
    if sensor_is_frozen(sensor_change_age) {
        return Some(LoopFault::SensorFrozen);
    }
    if sensor_is_stale(sensor_age) {
        return Some(LoopFault::SensorStale);
    }
    None
}

pub fn sensor_is_stale(age: Option<Duration>) -> bool {
    age.is_none_or(|age| age > SENSOR_TIMEOUT)
}

pub fn pid_from_settings(settings: PidSettings) -> PidController {
    let mut pid = PidController::default();
    apply_pid_settings(&mut pid, &settings);
    pid
}

pub fn apply_pid_settings(pid: &mut PidController, settings: &PidSettings) {
    pid.kp = settings.kp;
    pid.ki = settings.ki;
    pid.kd = settings.kd;
    pid.min_output = settings.min_output;
    pid.max_output = settings.max_output;
}

pub fn validate_pid_settings(settings: &PidSettings) -> Result<(), &'static str> {
    if ![
        settings.kp,
        settings.ki,
        settings.kd,
        settings.min_output,
        settings.max_output,
        settings.setpoint,
    ]
    .into_iter()
    .all(f64::is_finite)
    {
        return Err("all values must be finite");
    }
    if settings.min_output >= settings.max_output {
        return Err("minimum output must be less than maximum output");
    }
    Ok(())
}

/// Lists `/dev/serial/by-id/*` ahead of the kernel names. A suspend/resume
/// re-enumerates USB, and `ttyUSB0` can come back as `ttyUSB1`; the by-id path
/// carries the adapter's serial number, so a saved config still points at the
/// same physical device.
///
/// A hub that drops or re-enumerates its device can leave the by-id symlink
/// behind after the tty it points at is gone. Dangling entries are skipped so
/// selecting one cannot fail on a dead path - the device, if it came back,
/// reappears under its kernel name or a fresh by-id link.
pub fn stable_first(ports: Vec<serialport::SerialPortInfo>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev/serial/by-id") {
        names.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| std::fs::canonicalize(path).is_ok())
                .map(|path| path.to_string_lossy().into_owned()),
        );
        names.sort();
    }
    names.extend(ports.into_iter().map(|port| port.port_name));
    names
}
