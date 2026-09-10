use crate::history::History;
use eframe::egui::{self, Color32};
use egui_plot::{Legend, Line, Plot, PlotPoints};

#[derive(Clone, Copy)]
pub enum LinkState {
    Off,
    Wait,
    On,
}

impl LinkState {
    pub fn from_open(open: bool) -> Self {
        if open {
            Self::On
        } else {
            Self::Off
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Self::Off => Color32::from_gray(120),
            Self::Wait => Color32::from_rgb(230, 170, 60),
            Self::On => Color32::from_rgb(80, 200, 120),
        }
    }
}

/// Below this the side-by-side X/Y/Z layout stacks vertically instead.
pub const MIN_COLUMN_WIDTH: f32 = 190.0;

pub fn fits_columns(ui: &egui::Ui, count: usize) -> bool {
    ui.available_width() >= MIN_COLUMN_WIDTH * count as f32
}

/// `"-"` for an empty catalog field, the value itself otherwise.
pub fn dash_if_blank(value: &str) -> &str {
    if value.trim().is_empty() {
        "-"
    } else {
        value
    }
}

pub fn status_pill(ui: &mut egui::Ui, label: &str, state: LinkState) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 4.0, state.color());
    ui.colored_label(state.color(), label);
}

pub fn error_color(error_percent: f64) -> Color32 {
    match error_percent.abs() {
        value if !value.is_finite() => Color32::LIGHT_RED,
        value if value < 1.0 => Color32::from_rgb(80, 200, 120),
        value if value < 5.0 => Color32::from_rgb(230, 170, 60),
        _ => Color32::LIGHT_RED,
    }
}

/// Position of `output` inside `[min, max]`, clamped to 0..=1. Degenerate or
/// non-finite ranges collapse to 0 so the bar never renders garbage.
pub fn output_fraction(output: f64, min: f64, max: f64) -> f64 {
    let span = max - min;
    if !span.is_finite() || span <= 0.0 || !output.is_finite() {
        return 0.0;
    }
    ((output - min) / span).clamp(0.0, 1.0)
}

pub fn port_selector(ui: &mut egui::Ui, id: &str, selected: &mut String, ports: &[String]) {
    let selected_text = if selected.trim().is_empty() {
        "Select port"
    } else {
        selected.as_str()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            for port in ports {
                ui.selectable_value(selected, port.clone(), port);
            }
        });
}

pub fn show_plot(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    series: &[(&str, &History, Color32)],
    follow: bool,
    height: f32,
) {
    ui.label(egui::RichText::new(title).small());
    Plot::new(id)
        .legend(Legend::default())
        .height(height)
        .show(ui, |plot_ui| {
            if follow {
                plot_ui.set_auto_bounds(true);
            }
            for (name, history, color) in series {
                plot_ui.line(
                    Line::new(*name, PlotPoints::new(history.points().to_vec())).color(*color),
                );
            }
        });
}
