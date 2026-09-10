use crate::IgrfApp;
use chrono::{Datelike, Timelike};
use igrf_core::geomagnetism::{Coordinate, GeomagnetismCalculator, UtcDateTime};
use igrf_core::{contour_segments, MapGrid};

/// Fixed contour step in nT, matching the C# app's hardcoded
/// `ContourLevelStep = 2000` - no UI input for this.
const CONTOUR_LEVEL_STEP_NT: f64 = 2000.0;

impl IgrfApp {
    /// Calculate Magnetism
    pub(crate) fn calculate_manual_wmm(&mut self) {
        let result = (|| {
            let latitude = self.manual_lat;
            let longitude = self.manual_lon;
            let coordinate =
                Coordinate::new(latitude, longitude).map_err(|error| error.to_string())?;
            let now = chrono::Utc::now();
            let date = UtcDateTime::new(
                now.year(),
                now.month() as u8,
                now.day() as u8,
                now.hour() as u8,
                now.minute() as u8,
                now.second() as u8,
                now.timestamp_subsec_millis() as u16,
            )
            .map_err(|error| error.to_string())?;
            GeomagnetismCalculator::new()
                .try_calculate_at_altitude(coordinate, 0.0, date)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "date is outside WMM2025 validity (2025-2030)".to_owned())
        })();
        match result {
            Ok(value) => {
                self.manual_result = Some(value);
                self.manual_error = None;
                self.set_status("Manual WMM2025 calculation complete");
            }
            Err(error) => {
                self.manual_result = None;
                self.manual_error = Some(error);
            }
        }
    }

    /// Load Model
    pub(crate) fn browse_map_grid(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Select Geomagnetic Grid Data")
            .add_filter("Text files", &["txt"])
            .add_filter("All files", &["*"])
            .pick_file();
        let Some(path) = picked else {
            return;
        };
        self.map_grid_path = path.display().to_string();
        match MapGrid::load(&path) {
            Ok(grid) => {
                self.map_grid = Some(grid);
                self.map_grid_error = None;
                self.regenerate_contours();
                self.set_status("Geomagnetic grid map loaded");
            }
            Err(error) => {
                self.map_grid = None;
                self.map_contours = None;
                self.map_grid_error = Some(error.to_string());
            }
        }
    }

    /// Generate Model
    pub(crate) fn regenerate_contours(&mut self) {
        let Some(grid) = &self.map_grid else {
            self.map_grid_error = Some("load a grid file first".to_owned());
            return;
        };
        self.map_contours = Some(contour_segments(grid, CONTOUR_LEVEL_STEP_NT));
        self.map_view_generation += 1;
        self.map_grid_error = None;
    }
}
