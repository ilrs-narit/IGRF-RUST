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
    writer: Box<dyn Write + Send>,
    path: PathBuf,
    day: NaiveDate,
}

impl CsvLogger {
    /// `path` names the series, not the file: `sensor_log.csv` writes
    /// `logs/sensor_log_2026-08-21.csv`. An explicit directory is honoured.
    pub fn open(path: impl AsRef<Path>, header: &str) -> io::Result<Self> {
        let (directory, base_name) = series_parts(path.as_ref());
        let day = Local::now().date_naive();
        let (writer, path) = open_day(&directory, &base_name, day, header)?;
        Ok(Self {
            directory,
            base_name,
            header: header.to_owned(),
            writer: Box::new(writer),
            path,
            day,
        })
    }

    /// Identical to [`CsvLogger::open`], except rows are appended to `writer`
    /// instead of a file: the header is not written to the custom sink, and
    /// the sink serves until the first date rotation opens a real file. The
    /// series path still decides the rotation target. Performs no IO.
    pub fn with_writer(
        path: impl AsRef<Path>,
        header: &str,
        writer: Box<dyn Write + Send>,
    ) -> Self {
        let (directory, base_name) = series_parts(path.as_ref());
        let day = Local::now().date_naive();
        let path = dated_path(&directory, &base_name, day);
        Self {
            directory,
            base_name,
            header: header.to_owned(),
            writer,
            path,
            day,
        }
    }

    /// File currently being appended to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let today = Local::now().date_naive();
        if today != self.day {
            let (writer, path) = open_day(&self.directory, &self.base_name, today, &self.header)?;
            self.writer = Box::new(writer);
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

/// The directory and base name a series path like `logs/sensor_log.csv`
/// implies; `open` and `with_writer` agree on both so rotation targets the
/// same files either way.
fn series_parts(path: &Path) -> (PathBuf, String) {
    let base_name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "sensor_log".to_owned());
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("logs"),
    };
    (directory, base_name)
}

fn dated_path(directory: &Path, base_name: &str, day: NaiveDate) -> PathBuf {
    directory.join(format!("{base_name}_{}.csv", day.format("%Y-%m-%d")))
}

fn open_day(
    directory: &Path,
    base_name: &str,
    day: NaiveDate,
    header: &str,
) -> io::Result<(BufWriter<File>, PathBuf)> {
    fs::create_dir_all(directory)?;
    let path = dated_path(directory, base_name, day);
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
mod tests;
