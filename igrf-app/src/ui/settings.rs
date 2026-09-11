use super::*;

impl IgrfApp {
    pub(crate) fn show_lan_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.refresh_lan();
            }
            if self.lan_task.is_some() {
                ui.spinner();
                ui.label("applying...");
            }
        });
        if self.lan_profiles.is_empty() {
            ui.label("No wired NetworkManager profile found");
            return;
        }

        let selected = self.lan_selected.min(self.lan_profiles.len() - 1);
        self.lan_selected = selected;
        let labels: Vec<String> = self
            .lan_profiles
            .iter()
            .map(|profile| profile.label())
            .collect();
        egui::ComboBox::from_id_salt("lan-profile")
            .selected_text(labels[selected].clone())
            .show_ui(ui, |ui| {
                for (index, label) in labels.iter().enumerate() {
                    ui.selectable_value(&mut self.lan_selected, index, label);
                }
            });

        let target = self.lan_profiles[self.lan_selected].clone();
        ui.label(
            egui::RichText::new(format!(
                "now: {} {}",
                target.method,
                if target.addresses.is_empty() {
                    "--"
                } else {
                    &target.addresses
                }
            ))
            .small()
            .weak(),
        );
        if target.carries_default_route {
            ui.colored_label(
                Color32::LIGHT_RED,
                "carries the default route - locked to avoid cutting this machine off",
            );
            return;
        }

        ui.horizontal(|ui| {
            ui.label("Address");
            ui.add(egui::TextEdit::singleline(&mut self.lan_cidr).desired_width(130.0));
        });
        let busy = self.lan_task.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Apply static"))
                .clicked()
            {
                self.apply_lan_static();
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Use DHCP"))
                .clicked()
            {
                self.apply_lan_dhcp();
            }
        });
    }

    pub(crate) fn show_config_panel(&mut self, ui: &mut egui::Ui) {
        if ui.button("Load SystemConfig.json").clicked() {
            self.load_config();
        }
        if ui.button("Save SystemConfig.json").clicked() {
            self.save_config();
        }
        ui.label("CSV path");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.log_path).desired_width(150.0));
            if self.logger.is_some() {
                if ui.button("Stop").clicked() {
                    self.stop_logging();
                }
            } else if ui.button("Start").clicked() {
                self.start_logging();
            }
        });
        ui.label(if self.logger.is_some() {
            "CSV: logging"
        } else {
            "CSV: stopped"
        });
    }

    /// Show display panel: window/fullscreen, UI scale, and which monitor to use for fullscreen.
    pub(crate) fn show_display_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Window mode (applies on restart)").weak());
        ui.horizontal(|ui| {
            for mode in [DisplayMode::Window, DisplayMode::Fullscreen] {
                let label = match mode {
                    DisplayMode::Window => "Window",
                    DisplayMode::Fullscreen => "Fullscreen borderless",
                };
                if ui
                    .selectable_label(self.config.display.mode == mode, label)
                    .clicked()
                {
                    self.config.display.mode = mode;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("UI scale");
            ui.add(
                egui::DragValue::new(&mut self.config.display.ui_scale)
                    .speed(0.05)
                    .range(0.5..=3.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Fullscreen monitor");
            ui.add(
                egui::DragValue::new(&mut self.config.display.fullscreen_monitor)
                    .speed(0.1)
                    .range(0..=16),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Top inset (px)");
            ui.add(
                egui::DragValue::new(&mut self.config.display.top_inset)
                    .speed(1.0)
                    .range(0.0..=400.0),
            );
        });
        ui.label(
            egui::RichText::new(
                "Fullscreen with a UI scale above 1.0 suits an embedded 1024x600 touchscreen. \
                 Monitor 0 is the only screen on a single-panel kiosk.",
            )
            .small()
            .weak(),
        );
    }

    /// Setpoint command panel: where the field comes from and how fast it may
    /// change. Everything here is nanotesla.
    pub(crate) fn show_setpoint_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Source");
            for source in [
                SetpointSource::Manual,
                SetpointSource::Profile,
                SetpointSource::Socket,
            ] {
                if ui
                    .selectable_label(self.setpoint_source == source, source.label())
                    .clicked()
                {
                    self.setpoint_source = source;
                }
            }
        });

        let target = self.slew.target();
        let current = self.slew.current();
        let magnitude = |value: [f64; 3]| value.iter().map(|v| v * v).sum::<f64>().sqrt();
        ui.label(
            egui::RichText::new(format!(
                "now {:.1} nT -> target {:.1} nT",
                magnitude(current),
                magnitude(target)
            ))
            .monospace(),
        );
        if !self.slew.is_settled() {
            ui.label(egui::RichText::new("ramping").small().weak());
        }

        ui.horizontal(|ui| {
            ui.label("Slew nT/s");
            let response = ui.add(
                egui::DragValue::new(&mut self.slew_rate)
                    .speed(100.0)
                    .range(1.0..=1e6),
            );
            if response.changed() {
                self.config.setpoint_slew_nt_per_second = self.slew_rate;
                self.set_status(format!("Setpoint ramps at {:.0} nT/s", self.slew_rate));
            }
        });

        match self.setpoint_source {
            SetpointSource::Manual => {
                ui.separator();
                ui.label("Command |B| along the WMM direction");
                ui.horizontal(|ui| {
                    ui.label("|B| nT");
                    ui.add(
                        egui::DragValue::new(&mut self.manual_magnitude)
                            .speed(1000.0)
                            .range(0.0..=1e6),
                    );
                    if ui.button("Command").clicked() {
                        self.apply_manual_magnitude();
                    }
                });
                match self.manual_result {
                    Some(wmm) => ui.label(
                        egui::RichText::new(format!(
                            "direction D {:.2} deg, I {:.2} deg",
                            wmm.declination, wmm.inclination
                        ))
                        .small()
                        .weak(),
                    ),
                    None => ui.label(
                        egui::RichText::new("run the WMM2025 calculation for a direction")
                            .small()
                            .weak(),
                    ),
                };
                if let Some(error) = &self.manual_setpoint_error {
                    ui.colored_label(Color32::LIGHT_RED, error);
                }
                ui.label(
                    egui::RichText::new("per-axis setpoints stay editable on each axis card")
                        .small()
                        .weak(),
                );
            }
            SetpointSource::Profile => {
                ui.separator();
                ui.label("CSV: time_s,bx_nt,by_nt,bz_nt");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.profile_path).desired_width(150.0));
                    if ui.button("Load").clicked() {
                        self.load_setpoint_profile();
                    }
                });
                match &self.profile {
                    Some(profile) => {
                        let rows = profile.len();
                        let duration = profile.duration_s();
                        ui.label(
                            egui::RichText::new(format!("{rows} rows, {duration:.1}s"))
                                .small()
                                .weak(),
                        );
                        ui.horizontal(|ui| {
                            let running = self.profile_started.is_some();
                            if ui.button(if running { "Stop" } else { "Play" }).clicked() {
                                self.profile_started = (!running).then(Instant::now);
                            }
                            if let Some(started) = self.profile_started {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "t = {:.1}s",
                                        started.elapsed().as_secs_f64()
                                    ))
                                    .monospace(),
                                );
                            }
                        });
                    }
                    None => {
                        ui.label(egui::RichText::new("no profile loaded").small().weak());
                    }
                }
            }
            SetpointSource::Socket => {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("UDP port");
                    ui.add(
                        egui::DragValue::new(&mut self.setpoint_port)
                            .speed(1.0)
                            .range(1..=u16::MAX),
                    );
                    ui.label("Bind");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.setpoint_bind_address)
                            .desired_width(96.0),
                    )
                    .on_hover_text(
                        "Interface the listener accepts datagrams on. 127.0.0.1 keeps it on \
                         this machine. Anything else lets any host that can route here drive \
                         the coils, with no authentication.",
                    );
                    if self.setpoint_server.is_listening() {
                        if ui.button("Stop").clicked() {
                            self.stop_setpoint_server();
                        }
                    } else if ui.button("Listen").clicked() {
                        self.start_setpoint_server();
                    }
                });
                ui.label(
                    egui::RichText::new(if self.setpoint_server.is_listening() {
                        "listening - send \"bx,by,bz\" in nT, newest datagram wins"
                    } else {
                        "stopped"
                    })
                    .small()
                    .weak(),
                );
            }
        }
    }

    /// Sensor calibration. These belong to the physical unit in the cage, so
    /// re-fitting the ellipsoid must not need a rebuild.
    pub(crate) fn show_calibration_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("nT/count");
            ui.add(
                egui::DragValue::new(&mut self.calibration.count_to_nt)
                    .speed(0.001)
                    .range(1e-6..=1e6),
            );
        });
        ui.label(egui::RichText::new("Hard iron nT").small().weak());
        egui::Grid::new("hard-iron-grid")
            .num_columns(3)
            .show(ui, |ui| {
                for value in &mut self.calibration.hard_iron {
                    ui.add(egui::DragValue::new(value).speed(1.0));
                }
                ui.end_row();
            });
        ui.label(egui::RichText::new("Soft iron").small().weak());
        egui::Grid::new("soft-iron-grid")
            .num_columns(3)
            .show(ui, |ui| {
                for row in &mut self.calibration.soft_iron {
                    for value in row {
                        ui.add(egui::DragValue::new(value).speed(0.0001).max_decimals(6));
                    }
                    ui.end_row();
                }
            });
        for warning in [self.calibration_warning(), self.authority_warning()]
            .into_iter()
            .flatten()
        {
            ui.colored_label(Color32::LIGHT_RED, warning);
        }
        ui.label(
            egui::RichText::new("Save SystemConfig.json to keep these")
                .small()
                .weak(),
        );
    }
}
