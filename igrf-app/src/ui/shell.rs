use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ShellAction {
    Stop,
    Reset,
    Exit,
}

fn navigation(ui: &mut egui::Ui, active: &mut AppTab) -> Option<ShellAction> {
    let mut action = None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let tab_width = ((ui.available_width() - 388.0) / 3.0).clamp(100.0, 172.0);
        for tab in [AppTab::Control, AppTab::Model, AppTab::Settings] {
            let selected = *active == tab;
            let text = egui::RichText::new(tab.label())
                .strong()
                .size(15.0)
                .color(if selected {
                    Color32::from_rgb(8, 26, 32)
                } else {
                    Color32::from_rgb(168, 184, 201)
                });
            let button = egui::Button::new(text).selected(selected);
            let response = ui.add_sized([tab_width, 48.0], button);
            if response.clicked() {
                *active = tab;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_sized(
                    [152.0, 56.0],
                    egui::Button::new(
                        egui::RichText::new("STOP ALL")
                            .strong()
                            .size(16.0)
                            .color(Color32::WHITE),
                    )
                    .fill(STOP_RED),
                )
                .clicked()
            {
                action = Some(ShellAction::Stop);
            }
            ui.add_space(8.0);
            ui.separator();
            if ui
                .add_sized([112.0, 48.0], egui::Button::new("Master reset"))
                .clicked()
            {
                action = Some(ShellAction::Reset);
            }
            if ui
                .add_sized([60.0, 48.0], egui::Button::new("Exit"))
                .clicked()
            {
                action = Some(ShellAction::Exit);
            }
        });
    });
    ui.add_space(4.0);
    action
}

impl IgrfApp {
    pub(crate) fn show_tab_strip(&mut self, ui: &mut egui::Ui) {
        match navigation(ui, &mut self.active_tab) {
            Some(ShellAction::Stop) => self.stop_all(),
            Some(ShellAction::Reset) => self.master_reset(),
            Some(ShellAction::Exit) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_switches_tabs_and_stop_remains_reachable_below_inset() {
        for inset in [0.0, 120.0] {
            let ctx = egui::Context::default();
            let mut active = AppTab::Control;
            let mut frame = |events: Vec<egui::Event>| {
                let mut action = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1024.0, 600.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        if inset > 0.0 {
                            egui::Panel::top("inset").exact_size(inset).show(ui, |_| {});
                        }
                        egui::Panel::top("nav").show(ui, |ui| {
                            assert!(ui.cursor().top() >= inset);
                            action = navigation(ui, &mut active);
                            assert!(ui.min_rect().right() <= 1024.0);
                        });
                    },
                );
                output.textures_delta.clear();
                (active, action)
            };
            frame(vec![]);
            for (x, expected_tab, expected_action) in [
                (260.0, AppTab::Model, None),
                (440.0, AppTab::Settings, None),
                (940.0, AppTab::Settings, Some(ShellAction::Stop)),
                (80.0, AppTab::Control, None),
                (940.0, AppTab::Control, Some(ShellAction::Stop)),
            ] {
                let pos = egui::pos2(x, inset + 30.0);
                frame(vec![egui::Event::PointerMoved(pos)]);
                frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                let (tab, action) = frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                assert!(
                    tab == expected_tab,
                    "inset={inset} x={x} got={} expected={}",
                    tab.label(),
                    expected_tab.label()
                );
                assert_eq!(action, expected_action);
            }
        }
    }
}
