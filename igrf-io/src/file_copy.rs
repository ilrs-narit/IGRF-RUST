//! Chunked file copy that reports progress, for the log / drive transfer.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

/// Bytes read, written and reported per progress step.
const CHUNK_BYTES: usize = 1 << 20;

/// Copies `source` over `dest`, calling `progress` with `(copied, total)` bytes
/// before the first chunk and after every chunk, and returns the byte count.
///
/// The destination is replaced, like `std::fs::copy`, but the copy reports how
/// far it has come. A failure part-way leaves a partial destination.
///
/// # Errors
/// Any open/read/write failure. The source is opened first, so a missing
/// source fails before the destination is created or truncated.
pub fn copy_file_with_progress(
    source: &Path,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<u64> {
    let mut reader = File::open(source)?;
    let total = reader.metadata()?.len();
    let mut writer = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)?;
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut copied = 0u64;
    progress(copied, total);
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        copied += read as u64;
        progress(copied, total);
    }
    writer.flush()?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("igrf-copy-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_copy_moves_every_byte_and_replaces_the_destination() {
        let dir = temp_dir("replace");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.csv");
        let dest = dir.join("dest.csv");
        let content = vec![b'x'; CHUNK_BYTES + 5];
        fs::write(&source, &content).unwrap();
        fs::write(&dest, b"older and longer than the new content").unwrap();

        let copied = copy_file_with_progress(&source, &dest, |_, _| {}).unwrap();

        assert_eq!(copied, content.len() as u64);
        assert_eq!(fs::read(&dest).unwrap(), content);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn progress_starts_at_zero_and_ends_at_the_total() {
        let dir = temp_dir("progress");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.csv");
        let dest = dir.join("dest.csv");
        let content = vec![b'x'; CHUNK_BYTES + 5];
        fs::write(&source, &content).unwrap();

        let mut reports = Vec::new();
        copy_file_with_progress(&source, &dest, |copied, total| {
            reports.push((copied, total));
        })
        .unwrap();

        let total = content.len() as u64;
        assert_eq!(reports.first(), Some(&(0, total)));
        assert_eq!(reports.last(), Some(&(total, total)));
        assert!(
            reports.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "progress must not go backwards: {reports:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_source_reports_an_error_and_creates_no_destination() {
        let dir = temp_dir("missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("source.csv");
        let dest = dir.join("dest.csv");

        let result = copy_file_with_progress(&missing, &dest, |_, _| {});

        assert!(result.is_err());
        assert!(!dest.exists(), "a failed copy must not leave an empty file");
        let _ = fs::remove_dir_all(dir);
    }
}
