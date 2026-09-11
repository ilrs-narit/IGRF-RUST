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
    UI_INTERVAL,
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

impl eframe::App for IgrfApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_io();
        self.run_pid();
        self.tick_satellite_tracking();
        self.osk.set_visible(ctx.text_edit_focused());
        if ctx.input(|input| input.key_pressed(egui::Key::F11)) {
            self.fullscreen = !self.fullscreen;
            if self.fullscreen {
                // Drop decorations and fullscreen onto the named screen, so the
                // window covers the whole panel with no title bar - matching
                // how `Display.Mode = Fullscreen` builds the window at startup.
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::SetMonitor(
                    self.config.display.fullscreen_monitor,
                ));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
            }
        }
        ctx.request_repaint_after(UI_INTERVAL);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let top_inset = self.config.display.top_inset.max(0.0);
        if top_inset > 0.0 {
            egui::Panel::top("readout")
                .exact_size(top_inset)
                .show(ui, |ui| self.show_readout(ui));
        }
        egui::Panel::top("tab-strip").show(ui, |ui| {
            self.show_tab_strip(ui);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if top_inset == 0.0 {
                    status_pill(
                        ui,
                        "Sensor",
                        if !self.sensor_manager.is_open() {
                            LinkState::Off
                        } else if self.sensor_manager.parser().is_sensor_ready()
                            && !sensor_is_stale(self.sensor_age())
                        {
                            LinkState::On
                        } else {
                            LinkState::Wait
                        },
                    );
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
                }
                ui.label(format!("Status: {}", self.status));
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::LIGHT_RED, format!("Error: {error}"));
                    if ui.small_button("Dismiss").clicked() {
                        self.error = None;
                    }
                }
            });
        });
        match self.active_tab {
            AppTab::Control => {
                egui::CentralPanel::default().show(ui, |ui| {
                    self.show_control_columns(ui);
                });
            }
            AppTab::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            // The left column carries only short forms, so give
                            // it less width than the right, which holds the
                            // wider setpoint / calibration / logging panels.
                            ui.horizontal_top(|ui| {
                                let gap = ui.spacing().item_spacing.x;
                                let total = ui.available_width();
                                let left = ((total - gap) * 0.36).max(180.0);
                                let right = (total - gap - left).max(220.0);

                                ui.allocate_ui_with_layout(
                                    egui::vec2(left, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(left);
                                        egui::CollapsingHeader::new("Connections")
                                            .default_open(true)
                                            .show(ui, |ui| self.show_connection_panel(ui));
                                        egui::CollapsingHeader::new("LAN static IP")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_lan_panel(ui));
                                        egui::CollapsingHeader::new("Display")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_display_panel(ui));
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    egui::vec2(right, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(right);
                                        egui::CollapsingHeader::new("Setpoint command")
                                            .default_open(true)
                                            .show(ui, |ui| self.show_setpoint_panel(ui));
                                        egui::CollapsingHeader::new("Sensor calibration")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_calibration_panel(ui));
                                        egui::CollapsingHeader::new("Config / logging")
                                            .default_open(true)
                                            .show(ui, |ui| self.show_config_panel(ui));
                                        egui::CollapsingHeader::new("Saved log files")
                                            .default_open(false)
                                            .show(ui, |ui| self.show_log_files_panel(ui));
                                    },
                                );
                            });
                        });
                });
            }
            AppTab::Model => {
                egui::Panel::left("model-setup")
                    .resizable(true)
                    .default_size(300.0)
                    .size_range(240.0..=460.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui::CollapsingHeader::new("IGRF Model")
                                .default_open(true)
                                .show(ui, |ui| self.show_map_panel(ui));
                            egui::CollapsingHeader::new("Time")
                                .default_open(true)
                                .show(ui, |ui| self.show_time_panel(ui));
                            egui::CollapsingHeader::new("Manual Magnetism Calculator")
                                .default_open(true)
                                .show(ui, |ui| self.show_manual_panel(ui));
                            egui::CollapsingHeader::new("Satellite Position")
                                .default_open(true)
                                .show(ui, |ui| self.show_satellite_panel(ui));
                        });
                    });
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| self.show_model_result_panel(ui));
                });
            }
        }
    }
}
