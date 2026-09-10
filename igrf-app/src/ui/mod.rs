mod control;
mod drives;
mod logs;
mod model;
mod satellite;
mod settings;
mod shell;

use crate::cage;
use crate::control_helpers::sensor_is_stale;
use crate::satellite_ui::{OBJECT_TYPE_CHOICES, RCS_OPTIONS};
use crate::ui_helpers::{
    dash_if_blank, error_color, fits_columns, output_fraction, port_selector, show_plot,
    status_pill, LinkState,
};
use crate::{
    AppTab, FileRow, IgrfApp, SetpointSource, AXES, CONTOUR_LINE_COLOR, SATELLITE_COLORS, STOP_RED,
};
use eframe::egui::{self, Color32};
use egui_plot::{Legend, Line, Plot, PlotPoint, PlotPoints, Points, Text};
use igrf_core::satellite::{elevation_deg, PRESETS};
use igrf_core::{DisplayMode, FIRMWARE_MAX_OUTPUT};
use igrf_io::{list_drives, StoredTle};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Clear a remembered file selection once that file is no longer a plain file in
/// the freshly scanned list (deleted, renamed, or navigated away from).
fn prune_selection(selected: &mut Option<String>, rows: &[FileRow]) {
    if let Some(name) = selected {
        if !rows.iter().any(|row| !row.is_dir && &row.name == name) {
            *selected = None;
        }
    }
}

/// Byte count as a short human string (`1.4 MB`), for the log file list.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
