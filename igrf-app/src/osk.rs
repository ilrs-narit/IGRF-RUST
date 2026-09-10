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

#[cfg(target_os = "linux")]
mod platform {
    use std::process::{Child, Command, Stdio};

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
    }

    impl OnScreenKeyboard {
        pub fn new() -> Self {
            Self {
                child: None,
                visible: false,
            }
        }

        /// Show the keyboard while a text field is focused, hide it otherwise.
        /// Called every frame; does nothing while the state is unchanged.
        pub fn set_visible(&mut self, want: bool) {
            if want == self.visible {
                return;
            }
            if want {
                if self.child.is_some() {
                    self.call("Show");
                } else {
                    self.start();
                }
            } else if self.child.is_some() {
                self.call("Hide");
            }
            self.visible = want;
        }

        /// Launch `onboard` docked along the bottom edge. It shows itself on
        /// start, which matches `want == true`.
        fn start(&mut self) {
            self.child = Command::new("onboard")
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

        /// Ask the running instance to show or hide over DBus. Failures are
        /// ignored: a missing `gdbus`, or a keyboard that has not finished
        /// starting, must not take the control loop down.
        fn call(&self, method: &str) {
            let method_name = format!("{IFACE}.{method}");
            let _ = Command::new("gdbus")
                .args([
                    "call",
                    "--session",
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
