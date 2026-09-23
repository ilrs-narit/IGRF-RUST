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

// Arc-shared so the test can read the bytes after the sink moved into the logger.
#[derive(Clone, Default)]
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailWrite;

impl Write for FailWrite {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "disk full"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailFlush;

impl Write for FailFlush {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Other, "flush failed"))
    }
}

#[test]
fn a_custom_sink_receives_rows_but_no_header() {
    let dir = directory("custom-sink");
    let _ = fs::remove_dir_all(&dir);
    let series = dir.join("sensor_log.csv");
    let sink = SharedSink::default();
    let bytes = sink.0.clone();

    let mut logger = CsvLogger::with_writer(&series, "a,b", Box::new(sink));
    logger.write_line("1,2").unwrap();
    logger.write_line("3,4").unwrap();

    assert_eq!(bytes.lock().unwrap().as_slice(), b"1,2\n3,4\n");
    assert!(!series.exists(), "with_writer performs no IO");
}

#[test]
fn a_custom_sink_write_error_is_propagated() {
    let series = directory("custom-sink-write-fail").join("sensor_log.csv");
    let mut logger = CsvLogger::with_writer(&series, "a,b", Box::new(FailWrite));

    let error = logger.write_line("1,2").unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

#[test]
fn a_custom_sink_flush_error_is_propagated() {
    let series = directory("custom-sink-flush-fail").join("sensor_log.csv");
    let mut logger = CsvLogger::with_writer(&series, "a,b", Box::new(FailFlush));

    let error = logger.write_line("1,2").unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::Other);
}
