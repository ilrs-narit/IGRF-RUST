use crate::satellite_ui::{TrackedSat, OBJECT_TYPE_CHOICES, SEARCH_PER_PAGE};
use crate::IgrfApp;
use chrono::{Datelike, Timelike};
use igrf_core::geomagnetism::{Coordinate, GeomagnetismCalculator, UtcDateTime};
use igrf_core::satellite::{elevation_deg, split_dateline_segments, TleSet, PRESETS};
use igrf_io::{fetch_object_type, Credentials, StoredTle, TleStore};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const TLE_STORE_PATH: &str = "tle_data.db";
/// Points per satellite ground track / field-vs-time curve, spread across
/// one full orbital period. Fine enough to trace the antimeridian crossing
/// without dominating a once-a-second recompute.
const GROUND_TRACK_SAMPLES: usize = 121;

impl IgrfApp {
    /// Parses the draft TLE fields and appends a new tracked satellite,
    /// whether or not tracking is currently running. Kept even on a TLE
    /// parse error so a typo shows up in the list next to a message instead
    /// of just vanishing.
    pub(crate) fn add_satellite(&mut self) {
        let sat = TrackedSat::new(
            self.new_satellite_name.trim().to_owned(),
            self.new_tle_line1.trim().to_owned(),
            self.new_tle_line2.trim().to_owned(),
        );
        let error = sat.error.clone();
        self.tracked_satellites.push(sat);
        match error {
            Some(error) => self.set_error(format!("Satellite added, but TLE is invalid: {error}")),
            None => self.set_status("Satellite added"),
        }
    }

    pub(crate) fn remove_satellite(&mut self, index: usize) {
        if index < self.tracked_satellites.len() {
            self.tracked_satellites.remove(index);
        }
    }

    /// Starts the shared simulated clock driving every tracked satellite.
    pub(crate) fn start_satellite_tracking(&mut self) {
        if self.tracked_satellites.is_empty() {
            self.satellite_error = Some("Add at least one satellite first".to_owned());
            return;
        }
        self.satellite_tracking = true;
        self.sim_time_offset_s = 0.0;
        self.sim_last_tick = None;
        self.satellite_error = None;
        // Otherwise a satellite already above the mask at start would fire a
        // spurious AOS the instant tracking begins.
        for sat in &mut self.tracked_satellites {
            sat.was_visible = false;
        }
        self.set_status("Satellite tracking started");
    }

    pub(crate) fn stop_satellite_tracking(&mut self) {
        self.satellite_tracking = false;
    }

    /// Copies a preset's TLE into the "add satellite" draft fields, preferring
    /// an element set that a catalog fetch saved to `tle_data.db` (matched by
    /// NORAD catalog number) over the baked-in lines. Local file read only - no
    /// network - so a preset selection always reflects the last fetch.
    pub(crate) fn fill_draft_from_preset(&mut self, index: usize) {
        let Some(preset) = PRESETS.get(index).copied() else {
            return;
        };
        match preset
            .to_tle_set()
            .catalog_number()
            .ok()
            .and_then(stored_tle)
        {
            Some(stored) => {
                self.new_satellite_name = if stored.object_name.trim().is_empty() {
                    preset.name.to_owned()
                } else {
                    stored.object_name
                };
                self.new_tle_line1 = stored.line1;
                self.new_tle_line2 = stored.line2;
            }
            None => {
                self.new_satellite_name = preset.name.to_owned();
                self.new_tle_line1 = preset.line1.to_owned();
                self.new_tle_line2 = preset.line2.to_owned();
            }
        }
    }

    /// Rebuilds every tracked satellite whose NORAD catalog number has a
    /// different element set in `tle_data.db`. Returns how many were refreshed.
    /// Never creates the database, and leaves tracking as it found it - callers
    /// stop it so the operator restarts on the fresh elements.
    pub(crate) fn apply_stored_tles(&mut self) -> usize {
        if !std::path::Path::new(TLE_STORE_PATH).exists() {
            return 0;
        }
        let Ok(mut store) = TleStore::open(TLE_STORE_PATH) else {
            return 0;
        };
        let mut refreshed = 0;
        for sat in &mut self.tracked_satellites {
            let Some(catalog) = catalog_of(&sat.line1) else {
                continue;
            };
            let Ok(Some(stored)) = store.get(catalog) else {
                continue;
            };
            if stored.line1.trim() == sat.line1.trim() && stored.line2.trim() == sat.line2.trim() {
                continue;
            }
            *sat = TrackedSat::new(sat.name.clone(), stored.line1, stored.line2);
            refreshed += 1;
        }
        refreshed
    }

    /// Reads `tle_data.db` for which object types have rows, so the search knows
    /// whether the selected type has been fetched. Local read only; a missing DB
    /// just means "nothing fetched yet".
    pub(crate) fn refresh_fetched_types(&mut self) {
        self.sat_search.fetched_types.clear();
        if !std::path::Path::new(TLE_STORE_PATH).exists() {
            return;
        }
        let Ok(mut store) = TleStore::open(TLE_STORE_PATH) else {
            return;
        };
        for (_, gp_type) in OBJECT_TYPE_CHOICES {
            if store.has_object_type(gp_type).unwrap_or(false) {
                self.sat_search.fetched_types.push(gp_type.to_owned());
            }
        }
    }

    /// Function for checking whether the selected type has been fetched, and if so, run the search
    pub(crate) fn run_catalog_search(&mut self) {
        if !self.sat_search.is_selected_type_fetched() {
            self.sat_search.results.clear();
            self.sat_search.total = 0;
            return;
        }
        let mut store = match TleStore::open(TLE_STORE_PATH) {
            Ok(store) => store,
            Err(error) => {
                self.sat_search.error = Some(error.to_string());
                return;
            }
        };
        let filter = self.sat_search.build_filter();
        match store.search(&filter, self.sat_search.page, SEARCH_PER_PAGE) {
            Ok(page) => {
                // A narrowed filter can leave `page` past the end - clamp and
                // re-query once so the list is never blank with a non-zero total.
                let last_page = page.total.saturating_sub(1) / SEARCH_PER_PAGE;
                if page.rows.is_empty() && self.sat_search.page > last_page {
                    self.sat_search.page = last_page;
                    let refetched = store
                        .search(&filter, self.sat_search.page, SEARCH_PER_PAGE)
                        .unwrap_or(page);
                    self.sat_search.results = refetched.rows;
                    self.sat_search.total = refetched.total;
                } else {
                    self.sat_search.results = page.rows;
                    self.sat_search.total = page.total;
                }
                self.sat_search.error = None;
            }
            Err(error) => self.sat_search.error = Some(error.to_string()),
        }
    }

    /// "Fetch data": pull every object of the selected type from Space-Track on
    /// a worker thread and upsert into `tle_data.db`. `poll_type_fetch` picks up
    /// the result.
    pub(crate) fn spawn_type_fetch(&mut self) {
        if self.sat_search.fetch_task.is_some() {
            return;
        }
        let gp_type: &'static str = self.sat_search.selected_type();
        let (sender, receiver) = mpsc::channel();
        self.sat_search.fetch_task = Some(receiver);
        thread::spawn(move || {
            let _ = sender.send(run_type_fetch(gp_type));
        });
        self.set_status(format!("Fetching {gp_type} from Space-Track..."));
    }

    pub(crate) fn poll_type_fetch(&mut self) {
        let Some(receiver) = &self.sat_search.fetch_task else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.sat_search.fetch_task = None;
                match result {
                    Ok(count) => {
                        self.refresh_fetched_types();
                        self.sat_search.page = 0;
                        self.sat_search.last_filter_key.clear();
                        self.sat_search.error = None;
                        self.set_status(format!(
                            "Fetched {count} {} object(s) into tle_data.db",
                            self.sat_search.selected_type()
                        ));
                    }
                    Err(error) => self.sat_search.error = Some(format!("Space-Track: {error}")),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.sat_search.fetch_task = None;
                self.sat_search.error = Some("fetch ended without a result".to_owned());
            }
        }
    }

    // Get time
    pub(crate) fn simulated_time(&self) -> Option<UtcDateTime> {
        let offset =
            chrono::Duration::milliseconds((self.sim_time_offset_s * 1000.0).round() as i64);
        let now = chrono::Utc::now().checked_add_signed(offset)?;
        UtcDateTime::new(
            now.year(),
            now.month() as u8,
            now.day() as u8,
            now.hour() as u8,
            now.minute() as u8,
            now.second() as u8,
            now.timestamp_subsec_millis() as u16,
        )
        .ok()
    }

    /// Advances the shared simulated clock once a second (real time) and
    /// re-propagates every tracked satellite to it: position, field, ground
    /// track, field-vs-time samples, and the AOS/LOS edge against the
    /// ground station. Gated to 1 Hz already, so recomputing a whole orbit's
    /// worth of samples per satellite here is cheap - no extra caching.
    pub(crate) fn tick_satellite_tracking(&mut self) {
        if !self.satellite_tracking || self.tracked_satellites.is_empty() {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.sim_last_tick {
            if now.duration_since(last) < Duration::from_secs(1) {
                return;
            }
        }
        self.sim_last_tick = Some(now);
        self.sim_time_offset_s += self.sim_time_speed as f64;

        let Some(time) = self.simulated_time() else {
            self.satellite_error = Some("simulated time is out of range".to_owned());
            return;
        };
        self.satellite_error = None;

        let station_lat = self.station_lat;
        let station_lon = self.station_lon;
        let mask = self.elevation_mask_deg;
        let calculator = GeomagnetismCalculator::new();
        // Collected rather than overwritten, so two satellites crossing the
        // mask in the same tick both get reported instead of one clobbering
        // the other in the status bar.
        let mut aos_los_messages = Vec::new();

        for sat in &mut self.tracked_satellites {
            let Some(tracker) = &sat.tracker else {
                continue;
            };
            match tracker.position_at(time) {
                Ok(position) => {
                    sat.error = None;
                    sat.field = Coordinate::new(position.latitude, position.longitude)
                        .ok()
                        .and_then(|coordinate| {
                            calculator
                                .try_calculate_at_altitude(coordinate, position.altitude_km, time)
                                .ok()
                                .flatten()
                        });

                    let elevation = elevation_deg(station_lat, station_lon, position.ecef_km);
                    let visible = elevation >= mask;
                    if visible != sat.was_visible {
                        aos_los_messages.push(format!(
                            "{}: {} (elevation {elevation:.1} deg)",
                            sat.name,
                            if visible { "AOS" } else { "LOS" }
                        ));
                    }
                    sat.was_visible = visible;

                    match tracker.ground_track(time, GROUND_TRACK_SAMPLES) {
                        Ok(track) => {
                            sat.track_segments = split_dateline_segments(&track);
                            let period = tracker.orbital_period_minutes();
                            let samples = track.len().max(2);
                            sat.field_track = track
                                .iter()
                                .enumerate()
                                .filter_map(|(index, sample)| {
                                    let minutes =
                                        (index as f64 / (samples - 1) as f64 - 0.5) * period;
                                    let coordinate =
                                        Coordinate::new(sample.latitude, sample.longitude).ok()?;
                                    let field = calculator
                                        .try_calculate_at_altitude(
                                            coordinate,
                                            sample.altitude_km,
                                            time,
                                        )
                                        .ok()??;
                                    Some([minutes, field.total_intensity])
                                })
                                .collect();
                        }
                        Err(_) => {
                            sat.track_segments.clear();
                            sat.field_track.clear();
                        }
                    }
                    sat.position = Some(position);
                }
                Err(error) => {
                    sat.error = Some(error.to_string());
                    sat.position = None;
                    sat.field = None;
                    sat.track_segments.clear();
                    sat.field_track.clear();
                }
            }
        }

        if !aos_los_messages.is_empty() {
            self.set_status(aos_los_messages.join("; "));
        }
    }
}

/// The element set stored in `tle_data.db` for this catalog number, if "Update
/// TLE Info" has fetched it before. Any problem - no database yet, unreadable,
/// nothing stored - is flattened to `None` so a broken file never keeps the
/// baked-in presets from loading.
fn stored_tle(catalog: u64) -> Option<StoredTle> {
    // Reading must never create the database: a user who never fetches should
    // get nothing in their working directory.
    if !std::path::Path::new(TLE_STORE_PATH).exists() {
        return None;
    }
    let mut store = TleStore::open(TLE_STORE_PATH).ok()?;
    store.get(catalog).ok().flatten()
}

/// The NORAD catalog number in field 2 of a TLE line 1 (`1 25544U ...` -> 25544),
/// if it parses.
fn catalog_of(line1: &str) -> Option<u64> {
    TleSet::new(line1.trim(), "").catalog_number().ok()
}

/// Worker body for the catalog search's "Fetch data": log in to Space-Track,
/// fetch every object of `gp_type` (`PAYLOAD` / `ROCKET BODY` / ...), and upsert
/// them into `tle_data.db`. Returns the row count; failures are flattened to a
/// message the panel can show.
fn run_type_fetch(gp_type: &str) -> Result<usize, String> {
    let credentials = Credentials::from_env().map_err(|error| error.to_string())?;
    let mut store = TleStore::open(TLE_STORE_PATH).map_err(|error| error.to_string())?;
    fetch_object_type(&credentials, &mut store, gp_type).map_err(|error| error.to_string())
}
