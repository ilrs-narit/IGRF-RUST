//! On-screen keyboard control for the embedded touch panel.
//!
//! egui never tells the OS when a text field gains focus, so no on-screen
//! keyboard appears by itself. On top of that, the Wayland keyboards
//! (squeekboard, wvkbd) do not deliver key events to this X11/XWayland window
//! at all, so the keyboard has to be an X11 one: `onboard` types through
//! XTEST, which is the path that actually reaches the winit/X11 window.
//!
//! `onboard` is started lazily on the first text-field focus and then left
//! running, so later shows and hides are instant DBus calls instead of a
//! multi-second process start.
//!
//! Those DBus calls are handed to a worker thread rather than run inline. This
//! is called from the UI thread every frame, and `gdbus call` waits on the
//! session bus with no timeout: running it inline froze the whole app - sensor
//! stream and all - whenever the bus was slow or the keyboard was wedged,
//! which on the kiosk reads as "the keyboard came up and it hung".

#[cfg(target_os = "linux")]
mod platform {
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, Sender};

    const DEST: &str = "org.onboard.Onboard";
    const PATH: &str = "/org/onboard/Onboard/Keyboard";
    const IFACE: &str = "org.onboard.Onboard.Keyboard";

    /// Keyboard size on the 1024x600 panel, and the y offset that docks it
    /// along the bottom edge.
    const WIDTH: u32 = 1024;
    const HEIGHT: u32 = 260;
    const TOP: u32 = 600 - HEIGHT;

    pub struct OnScreenKeyboard {
        /// The instance this app started, if any. `None` until the first show.
        child: Option<Child>,
        visible: bool,
        onboard_bin: String,
        /// Serialises the `gdbus` show/hide calls on a worker thread. The UI
        /// thread only enqueues; it never waits for the session bus.
        requests: Option<Sender<&'static str>>,
    }

    impl OnScreenKeyboard {
        pub fn new() -> Self {
            Self::with_commands("onboard", "gdbus")
        }

        /// The binary names are injectable so a test can point them at scripts
        /// instead of the real keyboard and session bus.
        pub(super) fn with_commands(
            onboard_bin: impl Into<String>,
            gdbus_bin: impl Into<String>,
        ) -> Self {
            let gdbus_bin = gdbus_bin.into();
            let (sender, receiver) = mpsc::channel::<&'static str>();
            // One worker owns every DBus call, so Show/Hide keep the order the
            // UI asked for and a slow bus never stalls a frame.
            std::thread::spawn(move || {
                while let Ok(method) = receiver.recv() {
                    let method_name = format!("{IFACE}.{method}");
                    let _ = Command::new(&gdbus_bin)
                        .args([
                            "call",
                            "--session",
                            // `gdbus` otherwise waits the D-Bus default 25 s on a
                            // keyboard that is not answering.
                            "--timeout",
                            "2",
                            "--dest",
                            DEST,
                            "--object-path",
                            PATH,
                            "--method",
                            method_name.as_str(),
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            });
            Self {
                child: None,
                visible: false,
                onboard_bin: onboard_bin.into(),
                requests: Some(sender),
            }
        }

        /// Show the keyboard while a text field is focused, hide it otherwise.
        /// Called every frame; does nothing while the state is unchanged.
        pub fn set_visible(&mut self, want: bool) {
            if want == self.visible {
                return;
            }
            if want {
                if self.keyboard_alive() {
                    self.call("Show");
                } else {
                    self.start();
                }
            } else {
                // Hide even when this app did not start the keyboard: the
                // session's XDG autostart owns Onboard, and a second process
                // forwards `Show` to it but there is no equivalent for `Hide`.
                // A call that finds no keyboard is discarded by the worker.
                self.call("Hide");
            }
            self.visible = want;
        }

        /// True while the `onboard` instance this app started is still running.
        ///
        /// A child that has exited is reaped and forgotten, so a keyboard that
        /// died is restarted on the next focus instead of being sent DBus
        /// calls that can never land.
        fn keyboard_alive(&mut self) -> bool {
            let Some(child) = self.child.as_mut() else {
                return false;
            };
            match child.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child = None;
                    false
                }
            }
        }

        /// Launch `onboard` docked along the bottom edge. It shows itself on
        /// start, which matches `want == true`.
        fn start(&mut self) {
            self.child = Command::new(&self.onboard_bin)
                // Without this, GTK sees WAYLAND_DISPLAY and runs onboard as a
                // Wayland client, where its XInput/XTEST backend fails ("not an
                // X display") and no key ever reaches the X11 app.
                .env("GDK_BACKEND", "x11")
                .arg("-s")
                .arg(format!("{WIDTH}x{HEIGHT}"))
                .arg("-x")
                .arg("0")
                .arg("-y")
                .arg(TOP.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok();
        }

        /// Ask the running instance to show or hide over DBus. The call is
        /// queued for the worker thread, so this never blocks the caller.
        fn call(&self, method: &'static str) {
            if let Some(requests) = &self.requests {
                let _ = requests.send(method);
            }
        }
    }

    impl Drop for OnScreenKeyboard {
        fn drop(&mut self) {
            // Only the instance this app started; a keyboard the operator
            // launched by hand is left alone.
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    pub struct OnScreenKeyboard;

    impl OnScreenKeyboard {
        pub fn new() -> Self {
            Self
        }

        pub fn set_visible(&mut self, _want: bool) {}
    }
}

pub use platform::OnScreenKeyboard;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::OnScreenKeyboard;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    fn fake(dir: &std::path::Path, name: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nsleep 3\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// The UI thread must never wait on the keyboard or the session bus. A
    /// `gdbus` that does not answer used to freeze the whole app - the sensor
    /// stream behind it and all - until the frame came back.
    #[test]
    fn set_visible_does_not_block_the_caller() {
        let dir = std::env::temp_dir().join(format!("igrf-osk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut keyboard =
            OnScreenKeyboard::with_commands(fake(&dir, "onboard"), fake(&dir, "gdbus"));

        keyboard.set_visible(true); // spawns the fake keyboard
        let started = Instant::now();
        keyboard.set_visible(false); // used to run `gdbus ... .status()` inline
        let elapsed = started.elapsed();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            elapsed < Duration::from_secs(1),
            "set_visible blocked the UI thread for {elapsed:?}"
        );
    }
}
