use crate::IgrfApp;
use igrf_io::write_controller_packet;
use std::time::{Duration, Instant};

/// The C# build reopened the port after this long without a packet, so an
/// unattended run survives a USB hiccup. The watchdog above only stops the
/// coils; it never brings the link back.
const SENSOR_RECONNECT_AFTER: Duration = Duration::from_secs(15);
/// How often a reconnect is retried while the sensor stays silent.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(10);

impl IgrfApp {
    pub(crate) fn connect_sensor(&mut self) {
        if self.sensor_baud == 0 {
            self.set_error("Sensor baud must be greater than zero");
            return;
        }
        if self.sensor_port.trim().is_empty() {
            self.set_error("Sensor port is empty");
            return;
        }
        let baud = self.sensor_baud;
        match self.sensor_manager.connect(self.sensor_port.trim(), baud) {
            Ok(()) => {
                self.last_handshake = None;
                self.last_sensor_packet = None;
                self.last_sensor_packet_wall = None;
                self.last_sensor_change = None;
                self.last_sensor_raw = None;
                self.last_filter_setpoint = None;
                self.sensor_intended = true;
                self.set_status(format!(
                    "Sensor connected: {} @ {baud}",
                    self.sensor_port.trim()
                ));
            }
            Err(error) => self.set_error(format!("Sensor connect failed: {error}")),
        }
    }

    pub(crate) fn disconnect_sensor(&mut self) {
        self.sensor_manager.disconnect();
        self.last_sensor_packet = None;
        self.last_sensor_packet_wall = None;
        self.last_sensor_change = None;
        self.last_sensor_raw = None;
        self.last_filter_setpoint = None;
        self.sensor_intended = false;
        self.resume_pending = false;
        self.paused_by_watchdog = [false; 3];
        self.set_status("Sensor disconnected");
    }

    pub(crate) fn connect_controller(&mut self) {
        if self.controller_baud == 0 {
            self.set_error("Controller baud must be greater than zero");
            return;
        }
        if self.controller_port.trim().is_empty() {
            self.set_error("Controller port is empty");
            return;
        }
        let baud = self.controller_baud;
        match self
            .controller_manager
            .connect(self.controller_port.trim(), baud)
        {
            Ok(()) => {
                self.reset_controller_link_stats();
                self.controller_intended = true;
                self.set_status(format!(
                    "Controller connected: {} @ {baud}",
                    self.controller_port.trim()
                ))
            }
            Err(error) => self.set_error(format!("Controller connect failed: {error}")),
        }
    }

    pub(crate) fn disconnect_controller(&mut self) {
        // Stop driving before the port closes: the controller holds the last
        // output it was given, so a bare disconnect leaves the coils energised
        // and the integrators winding for the next connect.
        self.stop_all();
        self.controller_manager.disconnect();
        self.controller_intended = false;
        self.set_status("Controller disconnected");
    }

    /// Best-effort zero on the coils, shared by every path that stops driving
    /// them. The controller keeps the last packet it received, so anything that
    /// stops the loop has to send zeros first; a failed write means the link is
    /// gone anyway, so the port is closed.
    pub(crate) fn zero_outputs(&mut self) -> std::io::Result<()> {
        self.outputs = [0.0; 3];
        if !self.controller_manager.is_open() {
            return Ok(());
        }
        let result = write_controller_packet(&mut self.controller_manager, 0.0, 0.0, 0.0);
        if result.is_err() {
            self.controller_manager.disconnect();
        }
        result
    }

    pub(crate) fn connect_magson(&mut self) {
        if self.magson_port == 0 {
            self.set_error("Magson port must be greater than zero");
            return;
        }
        if self.magson_ip.trim().is_empty() {
            self.set_error("Magson IP/host is empty");
            return;
        }
        let port = self.magson_port;
        match self.magson_client.connect(self.magson_ip.trim(), port) {
            Ok(receiver) => {
                self.magson_receiver = Some(receiver);
                self.set_status(format!(
                    "Magson connected: {}:{port}",
                    self.magson_ip.trim()
                ));
            }
            Err(error) => self.set_error(format!("Magson connect failed: {error}")),
        }
    }

    pub(crate) fn disconnect_magson(&mut self) {
        self.magson_client.disconnect();
        self.magson_receiver = None;
        self.clear_magson();
        self.set_status("Magson disconnected");
    }

    /// Reopens the sensor port by itself, matching the C# watchdog: a run left
    /// alone for a month must survive a cable or driver hiccup without someone
    /// there to press Connect.
    pub(crate) fn maybe_reconnect_sensor(&mut self) {
        if !self.sensor_intended
            || !self
                .sensor_age()
                .is_none_or(|age| age > SENSOR_RECONNECT_AFTER)
        {
            return;
        }
        if self
            .last_reconnect
            .is_some_and(|last| last.elapsed() < RECONNECT_INTERVAL)
        {
            return;
        }
        self.last_reconnect = Some(Instant::now());
        self.sensor_manager.disconnect();
        self.connect_sensor();
        if self.sensor_manager.is_open() && self.resume_after_reconnect {
            // Wait for a real packet before driving the coils again; the
            // watchdog would only have to stop them a tick later otherwise.
            self.resume_pending = true;
        }
    }

    /// Reopens the controller port by itself, on the same terms as the sensor.
    ///
    /// More urgent than the sensor's: a sensor that is gone stops the loop and
    /// nothing moves, but a controller that is gone leaves six coils holding
    /// whatever they were last told, because the firmware never times out its
    /// receive. Reopening the port is the only thing that can zero them.
    pub(crate) fn maybe_reconnect_controller(&mut self) {
        if !self.controller_intended || self.controller_manager.is_open() {
            return;
        }
        if self
            .last_controller_reconnect
            .is_some_and(|last| last.elapsed() < RECONNECT_INTERVAL)
        {
            return;
        }
        self.last_controller_reconnect = Some(Instant::now());
        let port = self.controller_port.trim().to_owned();
        if self.controller_baud == 0 {
            return;
        }
        let baud = self.controller_baud;
        if self.controller_manager.connect(&port, baud).is_err() {
            return;
        }
        self.reset_controller_link_stats();
        // The link is back but the coils are still holding the last command the
        // firmware got. Zero them before anything decides whether to resume.
        if let Err(error) = self.zero_outputs() {
            self.set_error(format!(
                "Controller reopened but will not accept writes: {error}"
            ));
            return;
        }
        self.set_status(format!("Controller reconnected: {port}; outputs zeroed"));
        if self.resume_after_reconnect {
            self.resume_pending = true;
        }
    }
}
