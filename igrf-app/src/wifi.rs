//! Wi-Fi network picker: scan and connect through `nmcli`.
//!
//! NetworkManager stores the profile it creates on a successful connect and
//! rejoins it by itself after a reboot, so there is no "remembered networks"
//! handling here. Every nmcli call runs on a worker thread because a scan or a
//! connect takes seconds and the control loop must not wait for it.

use crate::netcfg::parse_terse;
use crate::IgrfApp;
use std::sync::mpsc;
use std::thread;

#[derive(Debug, Clone, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    /// 0-100, as reported by NetworkManager.
    pub signal: u8,
    pub secured: bool,
}

/// What a finished worker hands back to `poll_wifi_task`.
pub enum WifiEvent {
    Scanned {
        current: Option<String>,
        networks: Vec<WifiNetwork>,
    },
    Connected(String),
}

/// Parses `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list`.
///
/// Returns the SSID currently in use and the visible networks, strongest first.
/// Hidden networks (empty SSID) are dropped, and an SSID seen on several access
/// points appears once with its best signal.
pub fn parse_wifi_list(output: &str) -> (Option<String>, Vec<WifiNetwork>) {
    let mut current = None;
    let mut networks: Vec<WifiNetwork> = Vec::new();
    for row in parse_terse(output) {
        let [in_use, ssid, signal, security] = <[String; 4]>::try_from(row).unwrap_or_default();
        if ssid.is_empty() {
            continue;
        }
        if in_use == "*" {
            current = Some(ssid.clone());
        }
        let signal = signal.trim().parse().unwrap_or(0);
        let secured = !matches!(security.trim(), "" | "--");
        match networks.iter_mut().find(|network| network.ssid == ssid) {
            Some(known) => {
                known.signal = known.signal.max(signal);
                known.secured |= secured;
            }
            None => networks.push(WifiNetwork {
                ssid,
                signal,
                secured,
            }),
        }
    }
    networks.sort_by_key(|network| std::cmp::Reverse(network.signal));
    (current, networks)
}

#[cfg(target_os = "linux")]
fn nmcli(args: &[&str]) -> Result<String, String> {
    use std::process::Command;
    let output = Command::new("nmcli")
        .args(args)
        .output()
        .map_err(|error| format!("cannot run nmcli: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message.trim();
        Err(if message.is_empty() {
            format!("nmcli failed with {}", output.status)
        } else {
            message.to_owned()
        })
    }
}

#[cfg(not(target_os = "linux"))]
fn nmcli(_: &[&str]) -> Result<String, String> {
    Err("Wi-Fi setup is only implemented for Linux (NetworkManager)".to_owned())
}

fn scan() -> Result<WifiEvent, String> {
    let output = nmcli(&[
        "-t",
        "-f",
        "IN-USE,SSID,SIGNAL,SECURITY",
        "device",
        "wifi",
        "list",
        "--rescan",
        "yes",
    ])?;
    let (current, networks) = parse_wifi_list(&output);
    Ok(WifiEvent::Scanned { current, networks })
}

/// SSID and password go in as separate argv entries, never through a shell.
// ponytail: the password is visible in `ps` for the seconds nmcli runs; fine on
// a single-user kiosk, switch to `--ask` over stdin if that ever changes.
fn connect(ssid: &str, password: &str) -> Result<WifiEvent, String> {
    let mut args = vec!["--wait", "30", "device", "wifi", "connect", ssid];
    if !password.is_empty() {
        args.extend(["password", password]);
    }
    nmcli(&args)?;
    Ok(WifiEvent::Connected(ssid.to_owned()))
}

impl IgrfApp {
    fn spawn_wifi_task<F>(&mut self, task: F)
    where
        F: FnOnce() -> Result<WifiEvent, String> + Send + 'static,
    {
        if self.wifi_task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.wifi_task = Some(receiver);
        thread::spawn(move || {
            let _ = sender.send(task());
        });
    }

    pub(crate) fn scan_wifi(&mut self) {
        self.wifi_scanned = true;
        self.spawn_wifi_task(scan);
    }

    pub(crate) fn connect_wifi(&mut self) {
        let Some(ssid) = self.wifi_selected.clone() else {
            self.wifi_status = Some(Err("Select a network first".to_owned()));
            return;
        };
        let password = self.wifi_password.clone();
        self.wifi_status = Some(Ok(format!("Connecting to {ssid}...")));
        self.spawn_wifi_task(move || connect(&ssid, &password));
    }

    pub(crate) fn poll_wifi_task(&mut self) {
        let Some(receiver) = &self.wifi_task else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Wi-Fi task ended without a result".to_owned())
            }
        };
        self.wifi_task = None;
        match result {
            Ok(WifiEvent::Scanned { current, networks }) => {
                self.wifi_current = current;
                self.wifi_networks = networks;
                // A scan after a failed or successful connect leaves that
                // outcome on screen; only a stale "Connecting..." is cleared.
                if matches!(&self.wifi_status, Some(Ok(message)) if message.ends_with("...")) {
                    self.wifi_status = None;
                }
            }
            Ok(WifiEvent::Connected(ssid)) => {
                self.wifi_status = Some(Ok(format!("Connected to {ssid}")));
                self.wifi_password.clear();
                self.scan_wifi();
            }
            Err(error) => self.wifi_status = Some(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_list_parses_escaped_colons_hidden_rows_and_duplicates() {
        let output = "\
 :Lab\\:iot2:41:WPA2
*:Phone:78:WPA2 WPA3
 ::90:WPA2
 :Cafe:55:
 :Lab\\:iot2:63:WPA2
 :Phone:30:WPA2 WPA3
";
        let (current, networks) = parse_wifi_list(output);
        assert_eq!(current.as_deref(), Some("Phone"));
        let net = |ssid: &str, signal, secured| WifiNetwork {
            ssid: ssid.to_owned(),
            signal,
            secured,
        };
        assert_eq!(
            networks,
            vec![
                net("Phone", 78, true),
                net("Lab:iot2", 63, true),
                net("Cafe", 55, false),
            ]
        );
    }

    #[test]
    fn wifi_list_without_a_connection_has_no_current_network() {
        assert_eq!(parse_wifi_list(""), (None, Vec::new()));
    }
}
