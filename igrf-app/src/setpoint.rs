use crate::control_helpers::is_loopback;
use crate::{IgrfApp, SetpointSource};
use igrf_core::{field_from_magnitude, SetpointProfile};
use igrf_io::DEFAULT_BIND_ADDRESS;
use std::time::{Duration, Instant};

/// A commanded field with no fresh command for this long ramps back to zero.
/// An external propagator that dies would otherwise leave the coils holding
/// its last vector for as long as the app runs.
const SETPOINT_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);

impl IgrfApp {
    /// Commands a field vector through the slew limiter. Nothing in the app
    /// writes `pid_settings[..].setpoint` directly any more: every command,
    /// whatever its source, ramps.
    pub(crate) fn command_setpoint(&mut self, field_nt: [f64; 3]) {
        self.slew.command(field_nt);
    }

    /// Advances the ramp and publishes the result as the live setpoint.
    pub(crate) fn advance_setpoint(&mut self, dt: f64) {
        self.slew.rate_nt_per_second = self.config.setpoint_slew_nt_per_second;
        let current = self.slew.step(dt);
        for (settings, value) in self.pid_settings.iter_mut().zip(current) {
            settings.setpoint = value;
        }
    }

    /// Pulls the newest command from whichever source is live. Only the last
    /// datagram of a burst matters: a setpoint is state, not a queue to drain.
    pub(crate) fn poll_setpoint_source(&mut self) {
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

    pub(crate) fn start_setpoint_server(&mut self) {
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

    pub(crate) fn stop_setpoint_server(&mut self) {
        self.setpoint_server.disconnect();
        self.setpoint_receiver = None;
        if self.setpoint_source == SetpointSource::Socket {
            self.setpoint_source = SetpointSource::Manual;
        }
        self.set_status("Setpoint socket stopped; holding the last command");
    }

    pub(crate) fn load_setpoint_profile(&mut self) {
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
    pub(crate) fn apply_manual_magnitude(&mut self) {
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
}
