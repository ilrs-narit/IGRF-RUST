use chrono::{Local, NaiveDate};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Appends rows to `<dir>/<base>_<YYYY-MM-DD>.csv` and starts a new file when the
/// local date changes, matching the C# logger.
///
/// The rotation is not cosmetic: a month at 10 Hz is roughly 6.5 GB and 26
/// million rows in one file, past what a spreadsheet will open at all and past
/// what a naive `read_csv` will fit in memory.
pub struct CsvLogger {
    directory: PathBuf,
    base_name: String,
    header: String,
    writer: BufWriter<File>,
    path: PathBuf,
    day: NaiveDate,
}

impl CsvLogger {
    /// `path` names the series, not the file: `sensor_log.csv` writes
    /// `logs/sensor_log_2026-08-21.csv`. An explicit directory is honoured.
    pub fn open(path: impl AsRef<Path>, header: &str) -> io::Result<Self> {
        let path = path.as_ref();
        let base_name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "sensor_log".to_owned());
        let directory = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("logs"),
        };

        let day = Local::now().date_naive();
        let (writer, path) = open_day(&directory, &base_name, day, header)?;
        Ok(Self {
            directory,
            base_name,
            header: header.to_owned(),
            writer,
            path,
            day,
        })
    }

    /// File currently being appended to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let today = Local::now().date_naive();
        if today != self.day {
            let (writer, path) = open_day(&self.directory, &self.base_name, today, &self.header)?;
            self.writer = writer;
            self.path = path;
            self.day = today;
        }
        self.writer.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            self.writer.write_all(b"\n")?;
        }
        self.writer.flush()
    }
}

fn open_day(
    directory: &Path,
    base_name: &str,
    day: NaiveDate,
    header: &str,
) -> io::Result<(BufWriter<File>, PathBuf)> {
    fs::create_dir_all(directory)?;
    let path = directory.join(format!("{base_name}_{}.csv", day.format("%Y-%m-%d")));
    let write_header = !path.exists() || path.metadata()?.len() == 0;
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut writer = BufWriter::new(file);
    if write_header {
        writeln!(writer, "{header}")?;
        writer.flush()?;
    }
    Ok((writer, path))
}

/// Whether `name` is one dated segment the logger writes
/// (`<series>_YYYY-MM-DD.csv`), as opposed to any other file in the folder.
fn is_dated_segment(name: &str) -> bool {
    if name.contains(['/', '\\']) {
        return false;
    }
    let Some(stem) = name.strip_suffix(".csv") else {
        return false;
    };
    stem.rsplit_once('_')
        .is_some_and(|(_, date)| NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok())
}

/// Why a log segment cannot be deleted.
#[derive(Debug)]
pub enum LogDeleteError {
    /// The name is not a dated segment the logger writes.
    NotALogSegment,
    /// It is the segment the logger has open right now.
    BeingLogged,
    /// The filesystem refused the removal.
    Io(io::Error),
}

impl std::fmt::Display for LogDeleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotALogSegment => {
                write!(f, "not a dated log file written by the CSV logger")
            }
            Self::BeingLogged => {
                write!(
                    f,
                    "the file being logged cannot be deleted; stop logging first"
                )
            }
            Self::Io(error) => write!(f, "cannot remove the file: {error}"),
        }
    }
}

impl std::error::Error for LogDeleteError {}

/// The path `name` would be deleted at, or why it must not be deleted at all.
///
/// # Errors
/// [`LogDeleteError::NotALogSegment`] when `name` is not a dated segment, or
/// [`LogDeleteError::BeingLogged`] when it is the file the caller reported as
/// currently open.
pub fn deletable_log_path(
    directory: &Path,
    name: &str,
    active: Option<&Path>,
) -> Result<PathBuf, LogDeleteError> {
    if !is_dated_segment(name) {
        return Err(LogDeleteError::NotALogSegment);
    }
    let path = directory.join(name);
    if active == Some(path.as_path()) {
        return Err(LogDeleteError::BeingLogged);
    }
    Ok(path)
}

/// Deletes one dated log segment and returns the number of bytes freed. The
/// caller is responsible for asking the operator first: this is the one path
/// that destroys recorded history, and it never touches a file the logger did
/// not write or the segment being written right now.
///
/// # Errors
/// As [`deletable_log_path`], plus [`LogDeleteError::Io`] when the file cannot
/// be measured or removed.
pub fn delete_log_segment(
    directory: &Path,
    name: &str,
    active: Option<&Path>,
) -> Result<u64, LogDeleteError> {
    let path = deletable_log_path(directory, name, active)?;
    let bytes = fs::metadata(&path).map_err(LogDeleteError::Io)?.len();
    fs::remove_file(&path).map_err(LogDeleteError::Io)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("igrf-log-{}-{name}", std::process::id()))
    }

    #[test]
    fn writes_header_once_and_appends_snapshot_lines() {
        let dir = directory("append");
        let _ = fs::remove_dir_all(&dir);
        let series = dir.join("sensor_log.csv");

        let mut logger = CsvLogger::open(&series, "a,b").unwrap();
        let file = logger.path().to_path_buf();
        logger.write_line("1,2").unwrap();
        drop(logger);

        let mut logger = CsvLogger::open(&series, "a,b").unwrap();
        logger.write_line("3,4").unwrap();
        assert_eq!(logger.path(), file, "same day must reuse the same file");
        drop(logger);

        assert_eq!(fs::read_to_string(&file).unwrap(), "a,b\n1,2\n3,4\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn each_day_gets_its_own_file_with_its_own_header() {
        let dir = directory("rotate");
        let _ = fs::remove_dir_all(&dir);
        let first_day = NaiveDate::from_ymd_opt(2025, 8, 21).unwrap();
        let second_day = NaiveDate::from_ymd_opt(2025, 8, 22).unwrap();

        let (mut first, first_path) = open_day(&dir, "sensor_log", first_day, "a,b").unwrap();
        writeln!(first, "1,2").unwrap();
        first.flush().unwrap();
        let (mut second, second_path) = open_day(&dir, "sensor_log", second_day, "a,b").unwrap();
        writeln!(second, "3,4").unwrap();
        second.flush().unwrap();

        assert_ne!(first_path, second_path);
        assert!(
            first_path.ends_with("sensor_log_2025-08-21.csv"),
            "{first_path:?}"
        );
        assert!(
            second_path.ends_with("sensor_log_2025-08-22.csv"),
            "{second_path:?}"
        );
        assert_eq!(fs::read_to_string(&first_path).unwrap(), "a,b\n1,2\n");
        assert_eq!(fs::read_to_string(&second_path).unwrap(), "a,b\n3,4\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_dated_segments_written_by_the_logger_count_as_log_files() {
        assert!(is_dated_segment("sensor_log_2026-09-22.csv"));
        assert!(is_dated_segment("cage_a_2026-01-02.csv"));

        for name in [
            "sensor_log.csv",
            "setpoint_profile.csv",
            "sensor_log_2026-09-22.csv.bak",
            "sensor_log_2026-09-22.txt",
            "notes.txt",
            "sensor_log_2026-13-01.csv",
            "archive/sensor_log_2026-09-22.csv",
            "archive\\sensor_log_2026-09-22.csv",
        ] {
            assert!(!is_dated_segment(name), "{name}");
        }
    }

    #[test]
    fn deleting_a_segment_removes_only_that_file_and_reports_its_size() {
        let dir = directory("delete");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let keep = dir.join("sensor_log_2026-09-21.csv");
        let doomed = dir.join("sensor_log_2026-09-22.csv");
        fs::write(&keep, "a,b\n").unwrap();
        fs::write(&doomed, "a,b\n1,2\n").unwrap();

        let bytes = delete_log_segment(&dir, "sensor_log_2026-09-22.csv", None).unwrap();

        assert_eq!(bytes, 8);
        assert!(!doomed.exists());
        assert!(keep.exists(), "another day's segment must survive");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_segment_being_written_is_never_deleted() {
        let dir = directory("active-segment");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let active = dir.join("sensor_log_2026-09-22.csv");
        fs::write(&active, "a,b\n").unwrap();

        let result = delete_log_segment(&dir, "sensor_log_2026-09-22.csv", Some(&active));

        assert!(matches!(result, Err(LogDeleteError::BeingLogged)));
        assert!(active.exists(), "the open segment must survive");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_the_logger_did_not_write_is_never_deleted() {
        let dir = directory("foreign-file");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let foreign = dir.join("setpoint_profile.csv");
        fs::write(&foreign, "time_s,bx_nt\n0,1\n").unwrap();

        let result = delete_log_segment(&dir, "setpoint_profile.csv", None);

        assert!(matches!(result, Err(LogDeleteError::NotALogSegment)));
        assert!(foreign.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_segment_reports_the_io_error() {
        let dir = directory("missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let result = delete_log_segment(&dir, "sensor_log_2026-09-22.csv", None);

        assert!(matches!(result, Err(LogDeleteError::Io(_))));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_series_name_without_a_directory_lands_under_logs() {
        let dir = directory("bare");
        let _ = fs::remove_dir_all(&dir);
        let day = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let (_writer, path) = open_day(Path::new("logs"), "sensor_log", day, "a").unwrap();
        assert!(path.starts_with("logs"), "{path:?}");
        assert!(path.ends_with("sensor_log_2026-01-02.csv"), "{path:?}");
        let _ = fs::remove_file(path);
    }
}
