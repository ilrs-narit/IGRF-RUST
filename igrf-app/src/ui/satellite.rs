use super::*;

impl IgrfApp {
    /// Satellite Position group: SGP4 propagation and the TEME->geodetic
    /// conversion happen in `igrf_core::satellite`; this owns the tracked
    /// satellite list, the "add satellite" draft fields, the ground station
    /// used for AOS/LOS, and the simulated clock's speed/offset.
    pub(crate) fn show_satellite_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Add satellite").strong());
        egui::ComboBox::from_id_salt("satellite-preset")
            .selected_text(match self.new_satellite_preset {
                Some(index) => PRESETS[index].name,
                None => "-- Manual --",
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.new_satellite_preset.is_none(), "-- Manual --")
                    .clicked()
                {
                    self.new_satellite_preset = None;
                }
                for (index, preset) in PRESETS.iter().enumerate() {
                    if ui
                        .selectable_label(self.new_satellite_preset == Some(index), preset.name)
                        .clicked()
                    {
                        self.new_satellite_preset = Some(index);
                        // Prefers a TLE that a catalog fetch saved to
                        // `tle_data.db` over the baked-in preset lines.
                        self.fill_draft_from_preset(index);
                    }
                }
            });

        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.new_satellite_name);
        });
        ui.horizontal(|ui| {
            ui.label("Line 1");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_tle_line1)
                    .desired_width(300.0)
                    .font(egui::TextStyle::Monospace),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Line 2");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_tle_line2)
                    .desired_width(300.0)
                    .font(egui::TextStyle::Monospace),
            );
        });
        if ui.button("Add satellite").clicked() {
            self.add_satellite();
        }

        // Search Satellite (Space-Track) section: Full panel in fn:show_catalog_search
        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Search Satellite (Space-Track)").strong());
        self.show_catalog_search(ui);

        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Ground station (AOS/LOS)").strong());
        ui.horizontal(|ui| {
            ui.label("Latitude");
            ui.add(
                egui::DragValue::new(&mut self.station_lat)
                    .speed(0.01)
                    .range(-90.0..=90.0),
            );
            ui.label("Longitude");
            ui.add(
                egui::DragValue::new(&mut self.station_lon)
                    .speed(0.01)
                    .range(-180.0..=180.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Elevation mask (deg)");
            ui.add(
                egui::DragValue::new(&mut self.elevation_mask_deg)
                    .speed(0.1)
                    .range(0.0..=90.0),
            );
        });
        ui.label(
            egui::RichText::new("Save SystemConfig.json to keep the satellite list and station")
                .small()
                .weak(),
        );

        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            if self.satellite_tracking {
                if ui.button("Stop Tracking").clicked() {
                    self.stop_satellite_tracking();
                }
            } else if ui.button("Generate Results").clicked() {
                self.start_satellite_tracking();
            }
        });
        if let Some(error) = &self.satellite_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }

        // Simulated time speed slider, with a reset button and a display of the current simulated time.
        ui.separator();
        ui.label("Simulated time speed (seconds per real second)");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.sim_time_speed, -600..=600));
            if ui.button("Reset").clicked() {
                self.sim_time_speed = 0;
                self.sim_time_offset_s = 0.0;
                self.sim_last_tick = None;
            }
        });
        if let Some(time) = self.simulated_time() {
            ui.label(format!(
                "Simulated time: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                time.year, time.month, time.day, time.hour, time.minute, time.second
            ));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Tracked satellites").strong());
        if self.tracked_satellites.is_empty() {
            ui.label("No satellites yet. Add one above.");
        }
        let mut to_remove = None;
        for (index, sat) in self.tracked_satellites.iter().enumerate() {
            let color = SATELLITE_COLORS[index % SATELLITE_COLORS.len()];
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color, "\u{25cf}");
                    ui.label(egui::RichText::new(&sat.name).strong());
                    if ui.small_button("Remove").clicked() {
                        to_remove = Some(index);
                    }
                });
                if let Some(error) = &sat.error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                match (sat.position, sat.field) {
                    (Some(position), field) => {
                        egui::Grid::new(("satellite-result-grid", index))
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label("Latitude");
                                ui.label(format!("{:.3}", position.latitude));
                                ui.end_row();
                                ui.label("Longitude");
                                ui.label(format!("{:.3}", position.longitude));
                                ui.end_row();
                                ui.label("Altitude km");
                                ui.label(format!("{:.3}", position.altitude_km));
                                ui.end_row();
                                ui.label("Total Intensity");
                                match field.map(|f| f.total_intensity) {
                                    Some(value) => ui.label(format!("{value:.3}")),
                                    None => ui.label("--"),
                                };
                                ui.end_row();
                                ui.label("Elevation");
                                let elevation = elevation_deg(
                                    self.station_lat,
                                    self.station_lon,
                                    position.ecef_km,
                                );
                                ui.label(format!(
                                    "{elevation:.1} deg ({})",
                                    if sat.was_visible {
                                        "visible"
                                    } else {
                                        "below mask"
                                    }
                                ));
                                ui.end_row();
                            });
                    }
                    (None, _) => {
                        ui.label("No result yet - press \"Generate Results\".");
                    }
                }
            });
        }
        if let Some(index) = to_remove {
            self.remove_satellite(index);
        }
    }

    // Search the Space-Track catalog for satellites, filter, and show search results
    pub(crate) fn show_catalog_search(&mut self, ui: &mut egui::Ui) {
        let fetching = self.sat_search.fetch_task.is_some();

        ui.horizontal(|ui| {
            ui.label("Object type");
            let before = self.sat_search.object_type;
            egui::ComboBox::from_id_salt("sat-search-type")
                .selected_text(OBJECT_TYPE_CHOICES[self.sat_search.object_type].0)
                .show_ui(ui, |ui| {
                    for (index, (label, _)) in OBJECT_TYPE_CHOICES.iter().enumerate() {
                        ui.selectable_value(&mut self.sat_search.object_type, index, *label);
                    }
                });
            if self.sat_search.object_type != before {
                self.sat_search.page = 0;
            }

            let fetch = egui::Button::new(if fetching {
                "Fetching\u{2026}"
            } else {
                "Fetch data"
            });
            if ui
                .add_enabled(!fetching, fetch)
                .on_hover_text("Fetch every object of this type from Space-Track into tle_data.db")
                .clicked()
            {
                self.spawn_type_fetch();
            }
        });

        if let Some(error) = self.sat_search.error.clone() {
            ui.colored_label(Color32::LIGHT_RED, error);
        }

        if !self.sat_search.is_selected_type_fetched() {
            ui.add_space(4.0);
            ui.colored_label(Color32::YELLOW, "The data has not been updated");
            ui.label(
                egui::RichText::new("Click \"Fetch data\" to download this object type.")
                    .small()
                    .weak(),
            );
            return;
        }

        // Search filters over the fetched data
        egui::Grid::new("sat-search-filters")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Object name");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.object_name)
                        .hint_text("e.g. STARLINK"),
                );
                ui.end_row();

                ui.label("NORAD ID");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.norad_cat_id)
                        .hint_text("e.g. 25544"),
                );
                ui.end_row();

                ui.label("RCS size");
                egui::ComboBox::from_id_salt("sat-search-rcs")
                    .selected_text(RCS_OPTIONS[self.sat_search.rcs_size])
                    .show_ui(ui, |ui| {
                        for (index, label) in RCS_OPTIONS.iter().enumerate() {
                            ui.selectable_value(&mut self.sat_search.rcs_size, index, *label);
                        }
                    });
                ui.end_row();

                ui.label("Launch site");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.site)
                        .hint_text("e.g. Cape Canaveral"),
                );
                ui.end_row();

                ui.label("Country code");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.country_code)
                        .hint_text("e.g. USA"),
                );
                ui.end_row();

                ui.label("Launched before");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.launch_date)
                        .hint_text("yyyy-mm-dd"),
                );
                ui.end_row();

                ui.label("Decayed before");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sat_search.decay_date)
                        .hint_text("yyyy-mm-dd"),
                );
                ui.end_row();
            });
        if ui.button("Clear filters").clicked() {
            self.sat_search.object_name.clear();
            self.sat_search.norad_cat_id.clear();
            self.sat_search.rcs_size = 0;
            self.sat_search.site.clear();
            self.sat_search.country_code.clear();
            self.sat_search.launch_date.clear();
            self.sat_search.decay_date.clear();
        }

        // if input key changed (!= last_filter_key), reset the page and re-run the search
        let key = self.sat_search.filter_key();
        if key != self.sat_search.last_filter_key {
            self.sat_search.last_filter_key = key;
            self.sat_search.page = 0;
            self.run_catalog_search();
        }

        ui.add_space(4.0);
        let page = self.sat_search.page;
        let page_count = self.sat_search.page_count();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} result(s) \u{2014} page {} of {}",
                self.sat_search.total,
                page + 1,
                page_count
            ));
            if ui
                .add_enabled(page > 0, egui::Button::new("\u{2039} Prev"))
                .clicked()
            {
                self.sat_search.page -= 1;
                self.run_catalog_search();
            }
            if ui
                .add_enabled(page + 1 < page_count, egui::Button::new("Next \u{203a}"))
                .clicked()
            {
                self.sat_search.page += 1;
                self.run_catalog_search();
            }
        });

        let mut pending_add: Option<StoredTle> = None;
        egui::ScrollArea::vertical()
            .min_scrolled_height(400.0)
            .id_salt("sat-search-results")
            .show(ui, |ui| {
                for row in &self.sat_search.results {
                    let add = ui
                        .horizontal(|ui| {
                            let clicked = ui.small_button("Select").clicked();
                            ui.label(format!("{}  #{}", row.object_name, row.norad_cat_id));
                            clicked
                        })
                        .inner;
                    if add {
                        pending_add = Some(row.clone());
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "    {} \u{b7} RCS {} \u{b7} {} \u{b7} site {} \u{b7} launched {}{}",
                            dash_if_blank(&row.object_type),
                            dash_if_blank(&row.rcs_size),
                            dash_if_blank(&row.country_code),
                            dash_if_blank(&row.site),
                            dash_if_blank(&row.launch_date),
                            if row.decay_date.is_empty() {
                                String::new()
                            } else {
                                format!(" \u{b7} decayed {}", row.decay_date)
                            },
                        ))
                        .small()
                        .weak(),
                    );
                }
            });
        if let Some(row) = pending_add {
            if row.line1.trim().is_empty() || row.line2.trim().is_empty() {
                self.set_error(format!("{} has no TLE lines stored", row.object_name));
            } else {
                // Load the result to see the TLE lines before Add satellite
                self.new_satellite_preset = None;
                self.new_satellite_name = row.object_name.clone();
                self.new_tle_line1 = row.line1;
                self.new_tle_line2 = row.line2;
                self.set_status(format!(
                    "{} loaded into \"Add satellite\" \u{2014} press \"Add satellite\" to track it",
                    row.object_name
                ));
            }
        }
    }
}
