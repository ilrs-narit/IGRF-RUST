use super::*;

impl IgrfApp {
    pub(crate) fn show_connection_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh ports").clicked() {
                self.refresh_ports();
            }
            ui.label(format!("{} detected", self.available_ports.len()));
        });
        ui.separator();
        ui.label("Sensor serial");
        port_selector(
            ui,
            "sensor-port",
            &mut self.sensor_port,
            &self.available_ports,
        );
        ui.horizontal(|ui| {
            ui.label("Baud");
            ui.add(
                egui::DragValue::new(&mut self.sensor_baud)
                    .speed(100.0)
                    .range(1..=u32::MAX),
            );
            if ui.button("Connect").clicked() {
                self.connect_sensor();
            }
            if ui.button("Disconnect").clicked() {
                self.disconnect_sensor();
            }
        });
        ui.label(if self.sensor_manager.is_open() {
            if self.sensor_manager.parser().is_sensor_ready() {
                "Sensor: connected / ready"
            } else {
                "Sensor: connected / waiting for OK"
            }
        } else {
            "Sensor: disconnected"
        });
        ui.checkbox(
            &mut self.resume_after_reconnect,
            "Resume PID after auto-reconnect",
        )
        .on_hover_text(
            "Off: the loop stays paused until someone starts it. On: it restarts \
             the coils by itself once packets return - only for unattended runs.",
        );

        ui.separator();
        ui.label("Controller serial");
        port_selector(
            ui,
            "controller-port",
            &mut self.controller_port,
            &self.available_ports,
        );
        ui.horizontal(|ui| {
            ui.label("Baud");
            ui.add(
                egui::DragValue::new(&mut self.controller_baud)
                    .speed(100.0)
                    .range(1..=u32::MAX),
            );
            if ui.button("Connect").clicked() {
                self.connect_controller();
            }
            if ui.button("Disconnect").clicked() {
                self.disconnect_controller();
            }
        });
        ui.label(if self.controller_manager.is_open() {
            "Controller: connected"
        } else {
            "Controller: disconnected"
        });
        // Kept visible after a disconnect: a link bad enough to drop packets is
        // a link bad enough to drop entirely, and the count is the evidence.
        if self.controller_sent > 0 {
            let rate = 100.0 * self.controller_rejected as f64 / self.controller_sent as f64;
            let label = format!(
                "Rejected: {} / {} sent ({rate:.2}%)",
                self.controller_rejected, self.controller_sent
            );
            if self.controller_rejected == 0 {
                ui.label(label)
            } else {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 60), label)
            }
            .on_hover_text(
                "Packets the firmware answered with \"Error\\r\" because the CRC did not \
                 match. Those commands never reached the coils. Anything above zero is a \
                 cabling or baud problem, not a tuning one.",
            );
        }

        ui.separator();
        ui.label("Magson TCP");
        ui.horizontal(|ui| {
            ui.label("IP/host");
            ui.add(egui::TextEdit::singleline(&mut self.magson_ip).desired_width(120.0));
        });
        ui.horizontal(|ui| {
            ui.label("Port");
            ui.add(
                egui::DragValue::new(&mut self.magson_port)
                    .speed(1.0)
                    .range(1..=u16::MAX),
            );
            if ui.button("Connect").clicked() {
                self.connect_magson();
            }
            if ui.button("Disconnect").clicked() {
                self.disconnect_magson();
            }
        });
        ui.label(if self.magson_client.is_open() {
            "Magson: connected"
        } else {
            "Magson: disconnected"
        });
        // The frame layout is not confirmed (see the README), so a climbing
        // count is the difference between "this build ignores some types" and
        // "the stream is not being understood at all".
        let dropped = self.magson_client.dropped_frames();
        if dropped > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(220, 120, 60),
                format!("Undecoded frames: {dropped}"),
            )
            .on_hover_text(
                "Frames read but not decoded: types this build ignores, plus                  anything discarded while resynchronising after a lost byte.",
            );
        }
    }

    /// One X/Y/Z summary with tuning in a separate Details window. Always visible so
    /// nothing that can stop an axis hides behind navigation.
    pub(crate) fn axis_column(&mut self, ui: &mut egui::Ui, axis: usize) {
        let label = AXES[axis];
        let target = self.slew.target();
        let mut command = None;
        let mut pause = false;
        let details_id = egui::Id::new(("axis-details", axis));
        let mut details_open = ui
            .ctx()
            .data_mut(|data| data.get_temp::<bool>(details_id).unwrap_or(false));
        ui.group(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let running = self.pid_running[axis];
            ui.horizontal(|ui| {
                status_pill(ui, &format!("Axis {label}"), LinkState::from_open(running));
                if ui
                    .add_sized(
                        [68.0, 44.0],
                        egui::Button::new(if running { "Pause" } else { "Start" }),
                    )
                    .clicked()
                {
                    self.pid_running[axis] = !running;
                    if !self.pid_running[axis] {
                        pause = true;
                    }
                }
                if ui
                    .add_sized([64.0, 44.0], egui::Button::new("Details"))
                    .clicked()
                {
                    details_open = !details_open;
                }
            });

            let error = self.pid_settings[axis].setpoint - self.filtered[axis];
            let error_percent = [
                self.processed.error_per_x,
                self.processed.error_per_y,
                self.processed.error_per_z,
            ][axis];

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:+.3}", self.filtered[axis]))
                        .monospace()
                        .size(16.0)
                        .strong(),
                );
                ui.label(egui::RichText::new("nT").small());
                ui.label(format!("set {:+.1}", self.pid_settings[axis].setpoint));
            });
            ui.colored_label(
                error_color(error_percent),
                format!("err {error:+.2} ({error_percent:.2}%)"),
            );

            let fraction = output_fraction(
                self.outputs[axis],
                self.pid_settings[axis].min_output,
                self.pid_settings[axis].max_output,
            );
            // At the limit the loop has no authority left, so the reading looks
            // steady for the wrong reason. Say so instead of just filling the bar.
            let saturated = running
                && (self.outputs[axis] >= self.pid_settings[axis].max_output
                    || self.outputs[axis] <= self.pid_settings[axis].min_output);
            let mut bar = egui::ProgressBar::new(fraction as f32).text(if saturated {
                format!("output {:+.3}  SATURATED", self.outputs[axis])
            } else {
                format!("output {:+.3}", self.outputs[axis])
            });
            if saturated {
                bar = bar.fill(STOP_RED);
            }
            ui.add(bar);
        });
        let ctx = ui.ctx().clone();
        let top = ui
            .max_rect()
            .top()
            .max(self.config.display.top_inset + 80.0);
        let bounds = egui::Rect::from_min_max(egui::pos2(0.0, top), ctx.content_rect().max);
        egui::Window::new(format!("Axis {label} · Details"))
            .id(details_id)
            .open(&mut details_open)
            .collapsible(false)
            .default_width(560.0)
            .constrain_to(bounds)
            .vscroll(true)
            .show(&ctx, |ui| {
                ui.label(format!(
                    "Raw {:+.3} · Calibrated {:+.3} nT",
                    self.raw[axis], self.calibrated[axis]
                ));
                if ui
                    .add_sized([100.0, 44.0], egui::Button::new("Reset axis"))
                    .clicked()
                {
                    self.reset_axis(axis);
                }
                let ceiling = FIRMWARE_MAX_OUTPUT[axis];
                ui.columns(3, |cols| {
                    egui::Grid::new(format!("pid-grid-{axis}"))
                        .num_columns(2)
                        .striped(true)
                        .show(&mut cols[0], |ui| {
                            for (name, value) in [
                                ("Kp", &mut self.pid_settings[axis].kp),
                                ("Ki", &mut self.pid_settings[axis].ki),
                                ("Kd", &mut self.pid_settings[axis].kd),
                            ] {
                                ui.label(name);
                                ui.add(egui::DragValue::new(value).speed(0.1));
                                ui.end_row();
                            }
                        });

                    egui::Grid::new(format!("out-grid-{axis}"))
                        .num_columns(2)
                        .striped(true)
                        .show(&mut cols[1], |ui| {
                            ui.label("Setpoint");
                            let mut commanded = target[axis];
                            if ui
                                .add(egui::DragValue::new(&mut commanded).speed(1.0))
                                .changed()
                            {
                                let mut field = target;
                                field[axis] = commanded;
                                command = Some(field);
                            }
                            ui.end_row();
                            ui.label("Min out");
                            ui.add(
                                egui::DragValue::new(&mut self.pid_settings[axis].min_output)
                                    .speed(1.0)
                                    .range(-ceiling..=0.0),
                            );
                            ui.end_row();
                            ui.label("Max out")
                                .on_hover_text(format!("firmware ceiling {ceiling:.0}"));
                            ui.add(
                                egui::DragValue::new(&mut self.pid_settings[axis].max_output)
                                    .speed(1.0)
                                    .range(0.0..=ceiling),
                            );
                            ui.end_row();
                        });

                    egui::Grid::new(format!("filter-grid-{axis}"))
                        .num_columns(2)
                        .striped(true)
                        .show(&mut cols[2], |ui| {
                            for (name, value, speed) in [
                                ("Q proc", &mut self.filter_settings[axis].q, 0.05),
                                ("R meas", &mut self.filter_settings[axis].r, 1.0),
                                ("Spike", &mut self.filter_settings[axis].spike_nt, 50.0),
                            ] {
                                ui.label(name);
                                ui.add(egui::DragValue::new(value).speed(speed).range(1e-6..=1e9));
                                ui.end_row();
                            }
                        });
                });
            });
        ctx.data_mut(|data| data.insert_temp(details_id, details_open));
        if pause {
            // Pausing one axis stops that axis' PID, but the controller holds
            // whatever it was last sent for all three. Push a packet now with
            // this axis at zero instead of waiting for the next tick.
            self.pids[axis].hold();
            self.outputs[axis] = 0.0;
            self.write_outputs();
        }
        if let Some(field) = command {
            self.setpoint_source = SetpointSource::Manual;
            self.command_setpoint(field);
        }
    }

    pub(crate) fn show_control_columns(&mut self, ui: &mut egui::Ui) {
        if !fits_columns(ui, 3) {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    self.show_magson_strip(ui);
                    ui.separator();
                    for axis in 0..3 {
                        self.axis_column(ui, axis);
                    }
                    ui.separator();
                    self.show_plots_header(ui);
                    for axis in 0..3 {
                        self.axis_sensor_plot(ui, axis, 160.0);
                    }
                    ui.separator();
                    self.show_cage(ui, 240.0);
                    self.magnitude_plot(ui, 160.0);
                    self.magson_plot(ui, 160.0);
                });
            return;
        }

        let budget = ui.available_height().clamp(320.0, 900.0);
        let plot_h = ((budget - 90.0) / 3.0).clamp(60.0, 190.0);
        let cage_h = (budget - 80.0 - 2.0 * (plot_h + 16.0)).clamp(140.0, 240.0);

        ui.columns(3, |columns| {
            for axis in 0..3 {
                self.axis_column(&mut columns[0], axis);
                columns[0].add_space(2.0);
            }

            self.show_plots_header(&mut columns[1]);
            for axis in 0..3 {
                self.axis_sensor_plot(&mut columns[1], axis, plot_h);
            }

            self.show_cage(&mut columns[2], cage_h);
            let right_plot_h = ((columns[2].available_height() - 88.0) / 2.0).max(35.0);
            self.magnitude_plot(&mut columns[2], right_plot_h);
            self.magson_plot(&mut columns[2], right_plot_h);
            self.show_magson_strip(&mut columns[2]);
        });
    }

    pub(crate) fn show_magson_strip(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Magson").strong().small());
            for (label, value) in [
                ("X", self.magson[0]),
                ("Y", self.magson[1]),
                ("Z", self.magson[2]),
                ("|B|", self.magson_total),
            ] {
                ui.label(
                    egui::RichText::new(format!("{label} {value:+.3}"))
                        .monospace()
                        .small(),
                );
            }
        });
    }

    pub(crate) fn show_cage(&mut self, ui: &mut egui::Ui, max_size: f32) {
        // The view wants signed drive normalised against each axis' own limit,
        // not raw controller units.
        let drive = std::array::from_fn(|axis| {
            let settings = &self.pid_settings[axis];
            let span = settings.min_output.abs().max(settings.max_output.abs());
            if span > 0.0 {
                (self.outputs[axis] / span).clamp(-1.0, 1.0)
            } else {
                0.0
            }
        });
        egui::CollapsingHeader::new("Coil cage")
            .default_open(true)
            .show(ui, |ui| cage::show(ui, &mut self.cage, drive, max_size));
    }

    pub(crate) fn show_plots_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Live plots").strong().small());
            ui.checkbox(&mut self.follow_plots, "Follow live");
        });
    }

    pub(crate) fn axis_sensor_plot(&self, ui: &mut egui::Ui, axis: usize, height: f32) {
        show_plot(
            ui,
            &format!("sensor-plot-{axis}"),
            &format!("{} nT: setpoint vs measured", AXES[axis]),
            &[
                (
                    "Setpoint",
                    &self.history.sensor_setpoint[axis],
                    Color32::LIGHT_RED,
                ),
                (
                    "Measured",
                    &self.history.sensor_measured[axis],
                    Color32::LIGHT_BLUE,
                ),
            ],
            self.follow_plots,
            height,
        );
    }

    pub(crate) fn magnitude_plot(&self, ui: &mut egui::Ui, height: f32) {
        show_plot(
            ui,
            "sensor-magnitude-plot",
            "|B| nT: setpoint vs measured",
            &[
                (
                    "Setpoint",
                    &self.history.sensor_magnitude_setpoint,
                    Color32::LIGHT_RED,
                ),
                (
                    "Measured",
                    &self.history.sensor_magnitude_measured,
                    Color32::LIGHT_BLUE,
                ),
            ],
            self.follow_plots,
            height,
        );
    }

    pub(crate) fn magson_plot(&self, ui: &mut egui::Ui, height: f32) {
        show_plot(
            ui,
            "magson-plot",
            "Magson X/Y/Z/total (nT)",
            &[
                ("X", &self.history.magson[0], Color32::LIGHT_RED),
                ("Y", &self.history.magson[1], Color32::LIGHT_GREEN),
                ("Z", &self.history.magson[2], Color32::LIGHT_BLUE),
                ("Total", &self.history.magson[3], Color32::YELLOW),
            ],
            self.follow_plots,
            height,
        );
    }
}
