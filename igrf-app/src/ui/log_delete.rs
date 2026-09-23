use super::*;
use igrf_io::{deletable_log_path, delete_log_segment};

impl IgrfApp {
    /// The Delete button for the selected log segment, with the reason it is
    /// disabled shown on hover. Clicking only asks for confirmation; nothing
    /// is deleted until [`Self::delete_selected_log_file`] runs.
    pub(crate) fn show_delete_button(&mut self, ui: &mut egui::Ui) {
        let active = self.active_log_file();
        let directory = self.log_directory();
        let (enabled, hint) = if self.file_copy.is_some() {
            (false, Some("A copy is already running".to_owned()))
        } else {
            match self.log_files_sel.as_deref() {
                Some(name) => match deletable_log_path(&directory, name, active.as_deref()) {
                    Ok(_) => (true, None),
                    Err(error) => (false, Some(error.to_string())),
                },
                None => (false, Some("Select a log file to delete".to_owned())),
            }
        };
        let response = ui.add_enabled(enabled, egui::Button::new("Delete"));
        let response = match hint {
            Some(hint) => response.on_disabled_hover_text(hint),
            None => response,
        };
        if response.clicked() {
            self.log_delete_confirm = self.log_files_sel.clone();
        }
    }

    /// The confirmation modal for deleting the selected log segment: the one
    /// action in the Files tab that destroys recorded history, so it never
    /// happens on a single click. Confirming hands the file name to
    /// [`Self::delete_selected_log_file`]; the modal itself deletes nothing.
    pub(crate) fn show_delete_confirm(&mut self, ui: &mut egui::Ui) {
        let Some(name) = self.log_delete_confirm.clone() else {
            return;
        };
        let size = self
            .log_files
            .iter()
            .find(|row| row.name == name)
            .map(|row| row.size_bytes);
        let mut confirmed = false;
        let mut cancelled = false;
        let modal = egui::Modal::new(egui::Id::new("delete-log-file")).show(ui.ctx(), |ui| {
            ui.set_max_width(340.0);
            ui.label(egui::RichText::new("Delete log file?").strong());
            let detail = match size {
                Some(bytes) => format!("{name} ({})", human_size(bytes)),
                None => name.clone(),
            };
            ui.label(egui::RichText::new(detail).monospace());
            ui.label(
                egui::RichText::new(
                    "This cannot be undone. Copy the file to a USB drive first if it is still needed.",
                )
                .small()
                .weak(),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
        if confirmed {
            self.delete_selected_log_file();
        } else if cancelled
            || modal.backdrop_response.clicked()
            || ui.input(|input| input.key_pressed(egui::Key::Escape))
        {
            self.log_delete_confirm = None;
        }
    }

    /// Deletes the segment the confirmation modal approved and reports it in
    /// the transfer line. The guard rules live in `igrf_io`; this only
    /// re-reads the selection and rescans the folder.
    fn delete_selected_log_file(&mut self) {
        let Some(name) = self.log_delete_confirm.take() else {
            return;
        };
        let directory = self.log_directory();
        let active = self.active_log_file();
        match delete_log_segment(&directory, &name, active.as_deref()) {
            Ok(bytes) => {
                self.log_files_sel = None;
                self.transfer_status = format!("Deleted {name} ({})", human_size(bytes));
            }
            Err(error) => self.transfer_status = format!("Delete failed: {error}"),
        }
        self.refresh_log_files();
    }
}
