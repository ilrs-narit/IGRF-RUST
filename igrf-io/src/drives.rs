//! Enumerating the removable drives an operator can copy log files to or from.

/// A drive offered in the external-drive picker.
pub struct DriveInfo {
    /// Root path to start browsing from, e.g. `D:\` or `/media/pi/USB`.
    pub root: String,
    /// What the picker shows, e.g. `D:\ (removable)` or `USB  (/media/pi/USB)`.
    pub label: String,
}

// Drives for windows
#[cfg(windows)]
pub fn list_drives() -> Vec<DriveInfo> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
    }

    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for bit in 0..26u32 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let letter = (b'A' + bit as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
            2 => "removable",
            3 => "fixed",
            4 => "network",
            5 => "cd-rom",
            6 => "ram disk",
            _ => "unknown",
        };
        drives.push(DriveInfo {
            label: format!("{root} ({kind})"),
            root,
        });
    }
    drives.sort_by_key(|drive| !drive.label.contains("(removable)"));
    drives
}

// Drive for non-windows
#[cfg(not(windows))]
pub fn list_drives() -> Vec<DriveInfo> {
    use std::path::Path;

    let mut drives: Vec<DriveInfo> = std::fs::read_to_string("/proc/mounts")
        .map(|mounts| removable_mount_points(&mounts))
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let name = path
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or(path.as_str())
                .to_owned();
            DriveInfo {
                label: format!("{name}  ({path})"),
                root: path,
            }
        })
        .collect();

    if drives.is_empty() {
        for base in ["/media", "/run/media", "/mnt"] {
            match std::fs::read_dir(base) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            drives.push(DriveInfo {
                                label: entry.file_name().to_string_lossy().into_owned(),
                                root: entry.path().to_string_lossy().into_owned(),
                            });
                        }
                    }
                }
                Err(_) => continue,
            }
            if drives.is_empty() && Path::new(base).is_dir() {
                drives.push(DriveInfo {
                    root: base.to_owned(),
                    label: base.to_owned(),
                });
            }
        }
    }

    drives
}

/// Mount points from `/proc/mounts` content that look like removable media: a
/// `/dev/...` block device mounted under `/media`, `/run/media`, or `/mnt`.
#[cfg(any(unix, test))]
fn removable_mount_points(proc_mounts: &str) -> Vec<String> {
    let mut points = Vec::new();
    for line in proc_mounts.lines() {
        let mut fields = line.split_whitespace();
        let source = fields.next().unwrap_or_default();
        let Some(target) = fields.next() else {
            continue;
        };
        if !source.starts_with("/dev/") {
            continue;
        }
        let target = unescape_octal(target);
        let under_media = ["/media/", "/run/media/", "/mnt/"]
            .iter()
            .any(|base| target.starts_with(base));
        if under_media && !points.contains(&target) {
            points.push(target);
        }
    }
    points
}

/// `/proc/mounts` octal-escapes space, tab, newline and backslash in the path
/// field (`\040` for a space). Restore the original bytes.
#[cfg(any(unix, test))]
fn unescape_octal(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = (bytes[i] == b'\\')
            .then(|| bytes.get(i + 1..i + 4))
            .flatten()
            .filter(|digits| digits.iter().all(|b| (b'0'..=b'7').contains(b)));
        match escape {
            Some(digits) => {
                let value = (digits[0] - b'0') as u32 * 64
                    + (digits[1] - b'0') as u32 * 8
                    + (digits[2] - b'0') as u32;
                out.push(value as u8);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
