//! Copying one file at a time between the logs folder and a drive, on a worker
//! thread so a 230 MB log does not block the PID loop or the STOP button.

use crate::ui_helpers::human_size;
use crate::IgrfApp;
use igrf_io::copy_file_with_progress;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// What the copy worker reports while and after it runs.
enum CopyUpdate {
    /// Bytes copied so far, and the size of the whole file.
    Progress { copied: u64, total: u64 },
    /// The worker is done: `Ok` is the byte count, `Err` a message to show.
    Done(Result<u64, String>),
}

/// Why a copy could not be started.
#[derive(Debug)]
pub(crate) enum CopyStartError {
    /// The source path has no file name to copy under.
    NoFileName,
    /// The destination directory could not be created.
    DestDir(io::Error),
}

/// How a finished copy ended.
pub(crate) enum CopyOutcome {
    /// Bytes written.
    Copied(u64),
    /// A message to show the operator.
    Failed(String),
}

/// One copy in flight: what is being copied, where to, and how far it has come.
pub(crate) struct FileCopy {
    /// File name, as shown in the progress line.
    pub(crate) name: String,
    /// Directory the file is being copied into.
    dest_dir: PathBuf,
    /// Whether the destination already existed and is being replaced.
    replaced: bool,
    pub(crate) copied: u64,
    pub(crate) total: u64,
    updates: Receiver<CopyUpdate>,
}

impl FileCopy {
    /// Starts copying `source` into `dest_dir` on a worker thread. The caller
    /// is told up front whether the copy could start at all.
    pub(crate) fn start(source: &Path, dest_dir: &Path) -> Result<Self, CopyStartError> {
        let Some(name) = source.file_name() else {
            return Err(CopyStartError::NoFileName);
        };
        std::fs::create_dir_all(dest_dir).map_err(CopyStartError::DestDir)?;
        let dest = dest_dir.join(name);
        let replaced = dest.exists();
        let (sender, updates) = mpsc::channel();
        let source = source.to_path_buf();
        thread::spawn(move || {
            let result = copy_file_with_progress(&source, &dest, |copied, total| {
                let _ = sender.send(CopyUpdate::Progress { copied, total });
            });
            let _ = sender.send(CopyUpdate::Done(result.map_err(|error| error.to_string())));
        });
        Ok(Self {
            name: name.to_string_lossy().into_owned(),
            dest_dir: dest_dir.to_path_buf(),
            replaced,
            copied: 0,
            total: 0,
            updates,
        })
    }

    /// Drains the worker's messages into `copied`/`total`, and returns `Some`
    /// the first time the copy has finished.
    pub(crate) fn poll(&mut self) -> Option<CopyOutcome> {
        loop {
            match self.updates.try_recv() {
                Ok(CopyUpdate::Progress { copied, total }) => {
                    self.copied = copied;
                    self.total = total;
                }
                Ok(CopyUpdate::Done(Ok(bytes))) => return Some(CopyOutcome::Copied(bytes)),
                Ok(CopyUpdate::Done(Err(message))) => return Some(CopyOutcome::Failed(message)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    return Some(CopyOutcome::Failed(
                        "the copy ended without a result".to_owned(),
                    ))
                }
            }
        }
    }

    /// Status line for the finished copy, as the transfer panel shows it.
    pub(crate) fn outcome_message(&self, outcome: &CopyOutcome) -> String {
        match outcome {
            CopyOutcome::Copied(bytes) => format!(
                "{} {} ({}) to {}",
                if self.replaced { "Replaced" } else { "Copied" },
                self.name,
                human_size(*bytes),
                self.dest_dir.display()
            ),
            CopyOutcome::Failed(message) => format!("Copy failed: {message}"),
        }
    }
}

impl IgrfApp {
    /// Starts copying `source` into `dest_dir` on a worker thread and puts the
    /// progress line up. Does nothing while another copy is already running.
    pub(crate) fn start_file_copy(&mut self, source: PathBuf, dest_dir: PathBuf) {
        if self.file_copy.is_some() {
            return;
        }
        match FileCopy::start(&source, &dest_dir) {
            Ok(copy) => self.file_copy = Some(copy),
            Err(CopyStartError::NoFileName) => {
                self.transfer_status = "No file selected".to_owned();
            }
            Err(CopyStartError::DestDir(error)) => {
                self.transfer_status = format!("Cannot open {}: {error}", dest_dir.display());
            }
        }
    }

    /// Picks up copy progress and the final result, once per frame. Both file
    /// lists are rescanned on completion because either may hold the new copy.
    pub(crate) fn poll_file_copy(&mut self) {
        let Some(copy) = self.file_copy.as_mut() else {
            return;
        };
        let Some(outcome) = copy.poll() else {
            return;
        };
        let Some(copy) = self.file_copy.take() else {
            return;
        };
        self.transfer_status = copy.outcome_message(&outcome);
        self.refresh_log_files();
        self.refresh_ext_files();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("igrf-transfer-{}-{name}", std::process::id()))
    }

    fn wait_for(copy: &mut FileCopy) -> CopyOutcome {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(outcome) = copy.poll() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "copy did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_started_copy_finishes_with_the_whole_file_and_reports_progress() {
        let dir = temp_dir("worker");
        let _ = fs::remove_dir_all(&dir);
        let dest_dir = dir.join("usb");
        fs::create_dir_all(&dest_dir).unwrap();
        let source = dir.join("sensor_log_2026-09-22.csv");
        let content = vec![b'x'; 2_500_000];
        fs::write(&source, &content).unwrap();

        let mut copy = FileCopy::start(&source, &dest_dir).unwrap();
        assert_eq!(copy.name, "sensor_log_2026-09-22.csv");
        assert!(!copy.replaced);

        let outcome = wait_for(&mut copy);
        match outcome {
            CopyOutcome::Copied(bytes) => assert_eq!(bytes, content.len() as u64),
            CopyOutcome::Failed(message) => panic!("copy failed: {message}"),
        }
        assert_eq!(copy.total, content.len() as u64);
        assert_eq!(
            fs::read(dest_dir.join("sensor_log_2026-09-22.csv")).unwrap(),
            content
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_copy_into_something_that_is_not_a_directory_cannot_start() {
        let dir = temp_dir("not-a-dir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.csv");
        fs::write(&source, b"x").unwrap();

        let result = FileCopy::start(&source, &source);

        assert!(matches!(result, Err(CopyStartError::DestDir(_))));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_source_without_a_file_name_cannot_start() {
        let result = FileCopy::start(Path::new("/"), Path::new("/tmp"));

        assert!(matches!(result, Err(CopyStartError::NoFileName)));
    }
}
