use super::*;

impl IgrfApp {
    /// Top-level page navigation
    pub(crate) fn show_tab_strip(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for tab in [AppTab::Control, AppTab::Model, AppTab::Settings] {
                let selected = self.active_tab == tab;
                let button =
                    egui::Button::new(egui::RichText::new(tab.label()).strong().size(15.0))
                        .selected(selected)
                        .min_size(egui::vec2(120.0, 28.0));
                if ui.add(button).clicked() {
                    self.active_tab = tab;
                }
            }
        });
        ui.add_space(2.0);
        ui.separator();
    }

    pub(crate) fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("IGRF control");
            ui.separator();
            let sensor_age = self.sensor_age();
            status_pill(
                ui,
                "Sensor",
                if !self.sensor_manager.is_open() {
                    LinkState::Off
                } else if self.sensor_manager.parser().is_sensor_ready()
                    && !sensor_is_stale(sensor_age)
                {
                    LinkState::On
                } else {
                    LinkState::Wait
                },
            );
            if self.sensor_manager.is_open() {
                ui.label(
                    egui::RichText::new(match sensor_age {
                        Some(age) => format!("{:.1}s", age.as_secs_f64()),
                        None => "no data".to_owned(),
                    })
                    .small()
                    .weak(),
                );
            }
            status_pill(
                ui,
                "Controller",
                LinkState::from_open(self.controller_manager.is_open()),
            );
            status_pill(
                ui,
                "Magson",
                LinkState::from_open(self.magson_client.is_open()),
            );
            status_pill(ui, "CSV", LinkState::from_open(self.logger.is_some()));
            ui.separator();
            let running = self.pid_running.iter().filter(|state| **state).count();
            ui.label(format!("PID {running}/3 running"));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let stop = egui::Button::new(
                    egui::RichText::new("STOP ALL")
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(STOP_RED)
                .min_size(egui::vec2(110.0, 26.0));
                if ui.add(stop).clicked() {
                    self.stop_all();
                }
                if ui.button("Master reset").clicked() {
                    self.master_reset();
                }
                // In borderless fullscreen there is no title-bar close button,
                // so the app has to offer its own exit. Closing runs `Drop`,
                // which zeroes the coils before the process goes away.
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}
