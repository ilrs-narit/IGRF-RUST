use super::*;

impl IgrfApp {
    pub(crate) fn show_manual_panel(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("manual-wmm-grid")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Latitude");
                ui.add(
                    egui::DragValue::new(&mut self.manual_lat)
                        .speed(0.01)
                        .range(-90.0..=90.0),
                );
                ui.end_row();
                ui.label("Longitude");
                ui.add(
                    egui::DragValue::new(&mut self.manual_lon)
                        .speed(0.01)
                        .range(-180.0..=180.0),
                );
                ui.end_row();
            });
        if ui.button("Calculate Magnetism").clicked() {
            self.calculate_manual_wmm();
        }
        if let Some(error) = &self.manual_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(result) = self.manual_result {
            egui::Grid::new("manual-wmm-result")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Declination", result.declination),
                        ("Inclination", result.inclination),
                        ("Horizontal Intensity", result.horizontal_intensity),
                        ("Total Intensity", result.total_intensity),
                        ("X", result.x),
                        ("Y", result.y),
                        ("Z", result.z),
                    ] {
                        ui.label(label);
                        ui.label(format!("{value:.4}"));
                        ui.end_row();
                    }
                });
        }
    }

    /// Tab left side: IGRF Model group loaded from a text file, and a "Generate Model"
    pub(crate) fn show_map_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Load Model").clicked() {
                self.browse_map_grid();
            }
        });
        if !self.map_grid_path.is_empty() {
            ui.label(format!("File: {}", self.map_grid_path));
        }
        if let Some(error) = &self.map_grid_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    /// Time group: live UTC clock
    pub(crate) fn show_time_panel(&self, ui: &mut egui::Ui) {
        ui.label(format!(
            "UTC: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")
        ));
    }

    /// Right side of the IGRF Model tab: Magnetism Result
    pub(crate) fn show_model_result_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Geomagnetic field map");
        match &self.map_contours {
            Some(contours) if !contours.is_empty() => {
                let mut labels: Vec<(f64, [f64; 2])> = Vec::new();
                for segment in contours {
                    let already_labelled = labels
                        .iter()
                        .any(|(level, _)| (level - segment.level).abs() < f64::EPSILON);
                    if !already_labelled {
                        labels.push((
                            segment.level,
                            [
                                (segment.start[0] + segment.end[0]) / 2.0,
                                (segment.start[1] + segment.end[1]) / 2.0,
                            ],
                        ));
                    }
                }
                Plot::new(("model-map", self.map_view_generation))
                    .x_axis_label("Longitude")
                    .y_axis_label("Latitude")
                    .height(720.0)
                    .show(ui, |plot_ui| {
                        for segment in contours {
                            plot_ui.line(
                                Line::new(
                                    "Contour lines",
                                    PlotPoints::new(vec![segment.start, segment.end]),
                                )
                                .color(CONTOUR_LINE_COLOR),
                            );
                        }
                        for (level, point) in &labels {
                            plot_ui.text(
                                Text::new(
                                    format!("contour-label-{level}"),
                                    PlotPoint::new(point[0], point[1]),
                                    format!("{level:.0}"),
                                )
                                .color(CONTOUR_LINE_COLOR),
                            );
                        }
                        // Ground track + current position per tracked
                        // satellite. Colors match the list in the left
                        // panel, which doubles as the legend - a `.legend()`
                        // here would otherwise also pick up every contour
                        // segment above.
                        for (index, sat) in self.tracked_satellites.iter().enumerate() {
                            let color = SATELLITE_COLORS[index % SATELLITE_COLORS.len()];
                            // `.id(...)` overrides the id egui_plot would
                            // otherwise derive from `name` alone, which
                            // would collide if two entries share a name
                            // (e.g. the ISS preset added twice).
                            for (segment_index, segment) in sat.track_segments.iter().enumerate() {
                                plot_ui.line(
                                    Line::new(sat.name.clone(), PlotPoints::new(segment.clone()))
                                        .id(egui::Id::new((
                                            "satellite-track",
                                            index,
                                            segment_index,
                                        )))
                                        .color(color),
                                );
                            }
                            if let Some(position) = sat.position {
                                plot_ui.points(
                                    Points::new(
                                        sat.name.clone(),
                                        vec![[position.longitude, position.latitude]],
                                    )
                                    .id(egui::Id::new(("satellite-point", index)))
                                    .radius(5.0)
                                    .color(color),
                                );
                            }
                        }
                    });
            }
            Some(_) => {
                ui.label("No contour lines at this level step for the loaded grid.");
            }
            None => {
                ui.label("No geomagnetic field map generated yet.");
            }
        }

        if self
            .tracked_satellites
            .iter()
            .any(|sat| !sat.field_track.is_empty())
        {
            ui.add_space(8.0);
            ui.heading("Satellite field intensity vs time");
            Plot::new("satellite-field-vs-time")
                .x_axis_label("Minutes from simulated time")
                .y_axis_label("Total intensity (nT)")
                .height(280.0)
                .legend(Legend::default())
                .show(ui, |plot_ui| {
                    for (index, sat) in self.tracked_satellites.iter().enumerate() {
                        if sat.field_track.is_empty() {
                            continue;
                        }
                        let color = SATELLITE_COLORS[index % SATELLITE_COLORS.len()];
                        plot_ui.line(
                            Line::new(sat.name.clone(), PlotPoints::new(sat.field_track.clone()))
                                .id(egui::Id::new(("satellite-field", index)))
                                .color(color),
                        );
                    }
                });
        }
    }
}
