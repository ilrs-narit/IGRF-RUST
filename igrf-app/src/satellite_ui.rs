use igrf_core::geomagnetism::GeomagnetismResult;
use igrf_core::satellite::{SatellitePosition, SatelliteTracker};
use igrf_core::SatelliteEntry;
use igrf_io::{StoredTle, TleFilter};
use std::sync::mpsc::Receiver;

/// One entry in the "Satellite Position" list: the TLE text plus everything
/// derived from it each tick. The tracker isn't serializable, so it's
/// rebuilt from `name`/`line1`/`line2` on add and on config load rather than
/// persisted itself - see [`igrf_core::SatelliteEntry`].
pub struct TrackedSat {
    pub name: String,
    pub line1: String,
    pub line2: String,
    pub tracker: Option<SatelliteTracker>,
    pub position: Option<SatellitePosition>,
    pub field: Option<GeomagnetismResult>,
    /// Ground track for the current simulated time, one full orbital period
    /// centered on it. `[longitude, latitude]` pairs, already split at the
    /// antimeridian - see [`split_dateline_segments`].
    pub track_segments: Vec<Vec<[f64; 2]>>,
    /// Total field intensity across the same track, as
    /// `[minutes_from_now, nT]` pairs for the field-vs-time plot.
    pub field_track: Vec<[f64; 2]>,
    /// Elevation above the ground station's horizon last tick, so an AOS/LOS
    /// status message fires on the transition instead of every tick.
    pub was_visible: bool,
    pub error: Option<String>,
}

impl TrackedSat {
    pub fn new(name: String, line1: String, line2: String) -> Self {
        let label = if name.trim().is_empty() {
            "Satellite".to_owned()
        } else {
            name.clone()
        };
        let (tracker, error) = match SatelliteTracker::from_tle(Some(&label), &line1, &line2) {
            Ok(tracker) => (Some(tracker), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            name: label,
            line1,
            line2,
            tracker,
            position: None,
            field: None,
            track_segments: Vec::new(),
            field_track: Vec::new(),
            was_visible: false,
            error,
        }
    }

    pub fn from_entry(entry: &SatelliteEntry) -> Self {
        Self::new(entry.name.clone(), entry.line1.clone(), entry.line2.clone())
    }

    pub fn to_entry(&self) -> SatelliteEntry {
        SatelliteEntry {
            name: self.name.clone(),
            line1: self.line1.clone(),
            line2: self.line2.clone(),
        }
    }
}

/// RCS-size live-filter options; index 0 is "any", the rest map to the stored
/// upper-case values.
pub const RCS_OPTIONS: [&str; 4] = ["(any)", "Small", "Medium", "Large"];
/// The catalog-search "header": the four fetchable GP object types. `.1` is the
/// Space-Track `OBJECT_TYPE` value. There is no "any" - a type must be picked
/// and fetched before the other filters apply (see `db.md`).
pub const OBJECT_TYPE_CHOICES: [(&str, &str); 4] = [
    ("Payload", "PAYLOAD"),
    ("Rocket Body", "ROCKET BODY"),
    ("Debris", "DEBRIS"),
    ("Unknown", "UNKNOWN"),
];
pub const SEARCH_PER_PAGE: usize = 10;

/// "Search catalog" state under the Satellite Position panel.
#[derive(Default)]
pub struct SatSearchState {
    /// Index into [`OBJECT_TYPE_CHOICES`] - the header selection.
    pub object_type: usize,

    // Live filters over the fetched rows.
    pub object_name: String,
    pub norad_cat_id: String,
    pub rcs_size: usize,
    pub site: String,
    pub country_code: String,
    /// `yyyy-mm-dd`; keeps results launched *before* this.
    pub launch_date: String,
    /// `yyyy-mm-dd`; keeps results decayed *before* this.
    pub decay_date: String,

    /// The running "Fetch data" worker, if any: sends back the row count or an
    /// error message.
    pub fetch_task: Option<Receiver<Result<usize, String>>>,
    /// GP object-type values that have at least one row stored (so the search
    /// can apply). Refreshed at startup and after every fetch.
    pub fetched_types: Vec<String>,

    pub results: Vec<StoredTle>,
    pub total: usize,
    pub page: usize,
    pub error: Option<String>,
    pub last_filter_key: String,
}

impl SatSearchState {
    /// The Space-Track `OBJECT_TYPE` value currently selected.
    pub fn selected_type(&self) -> &'static str {
        OBJECT_TYPE_CHOICES[self.object_type].1
    }

    pub fn is_selected_type_fetched(&self) -> bool {
        self.fetched_types.iter().any(|t| t == self.selected_type())
    }

    /// The filter for the live search: always scoped to the selected object
    /// type, plus whichever other fields are filled in.
    pub fn build_filter(&self) -> TleFilter {
        let text = |value: &str| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        };
        TleFilter {
            object_type: Some(self.selected_type().to_owned()),
            object_name: text(&self.object_name),
            norad_cat_id: text(&self.norad_cat_id),
            rcs_size: (self.rcs_size > 0).then(|| RCS_OPTIONS[self.rcs_size].to_uppercase()),
            site: text(&self.site),
            country_code: text(&self.country_code),
            launch_date_before: text(&self.launch_date),
            decay_date_before: text(&self.decay_date),
        }
    }

    /// Filter key for the search
    pub fn filter_key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            self.object_type,
            self.object_name.trim(),
            self.norad_cat_id.trim(),
            self.rcs_size,
            self.site.trim(),
            self.country_code.trim(),
            self.launch_date.trim(),
            self.decay_date.trim(),
        )
    }

    pub fn page_count(&self) -> usize {
        self.total.div_ceil(SEARCH_PER_PAGE).max(1)
    }
}
