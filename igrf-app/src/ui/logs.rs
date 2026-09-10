use super::*;

impl IgrfApp {
    // Get the logs directory
    pub(crate) fn log_directory(&self) -> PathBuf {
        match Path::new(self.log_path.trim()).parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("logs"),
        }
    }

    /// Rescan the log directory for the "Saved log files" list. Read-only: a
    /// missing directory is reported, not created.
    pub(crate) fn refresh_log_files(&mut self) {
        let dir = self.log_directory();
        self.log_files.clear();
        self.log_files_scanned = true;
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    match entry.metadata() {
                        Ok(meta) if meta.is_file() => self.log_files.push(FileRow {
                            name: entry.file_name().to_string_lossy().into_owned(),
                            is_dir: false,
                            size_bytes: meta.len(),
                        }),
                        _ => {}
                    }
                }
                // Sort files by name descending
                self.log_files.sort_by(|a, b| b.name.cmp(&a.name));
                self.log_files_status = format!("{} item(s)", self.log_files.len());
                prune_selection(&mut self.log_files_sel, &self.log_files);
            }
            Err(error) => {
                self.log_files_status = format!("Cannot read {}: {error}", dir.display());
            }
        }
    }

    // Show log files panel: left column is the logs folder, right column is the external drive
    pub(crate) fn show_log_files_panel(&mut self, ui: &mut egui::Ui) {
        if !self.log_files_scanned {
            self.refresh_log_files();
        }
        if !self.ext_scanned {
            self.refresh_drives();
            self.ext_scanned = true;
        }
        let logs_path = self.log_directory().display().to_string();
        let drive_path = self
            .ext_cwd
            .as_deref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "(no drive selected)".to_owned());

        ui.horizontal_top(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let mid = 54.0;
            let side = ((ui.available_width() - mid - gap * 2.0) / 2.0).max(140.0);

            ui.allocate_ui_with_layout(
                egui::vec2(side, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(side);
                    ui.strong("Logs folder");
                    ui.label(egui::RichText::new(&logs_path).monospace().small());
                    self.show_logs_folder_list(ui);
                },
            );

            // Middle strip, between the two lists
            ui.allocate_ui_with_layout(
                egui::vec2(mid, 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_width(mid);
                    self.show_transfer_buttons(ui);
                },
            );

            ui.allocate_ui_with_layout(
                egui::vec2(side, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(side);
                    ui.strong("External drive");
                    ui.label(egui::RichText::new(&drive_path).monospace().small());
                    self.show_external_drive_panel(ui);
                },
            );
        });

        if !self.transfer_status.is_empty() {
            ui.label(egui::RichText::new(&self.transfer_status).small().weak());
        }
    }

    /// The copy buttons in the strip between the two lists: '>>' and '<<'
    pub(crate) fn show_transfer_buttons(&mut self, ui: &mut egui::Ui) {
        let to_logs_ready = self.ext_sel.is_some();
        let to_drive_ready = self.log_files_sel.is_some() && self.ext_cwd.is_some();
        let size = egui::vec2(40.0, 30.0);

        ui.add_space(96.0);
        if ui
            .add_enabled(to_drive_ready, egui::Button::new(">>").min_size(size))
            .on_hover_text("Copy the selected log file into the open drive folder")
            .clicked()
        {
            self.copy_logs_to_drive();
        }
        ui.add_space(8.0);
        if ui
            .add_enabled(to_logs_ready, egui::Button::new("<<").min_size(size))
            .on_hover_text("Copy the selected drive file into the logs folder")
            .clicked()
        {
            self.copy_drive_to_logs();
        }
    }

    /// Left column: ls every file in the log directory, with a Refresh to
    /// rescan. Click a file to select it for a `>>` copy.
    pub(crate) fn show_logs_folder_list(&mut self, ui: &mut egui::Ui) {
        if ui.button("Refresh").clicked() {
            self.refresh_log_files();
        }
        ui.label(egui::RichText::new(&self.log_files_status).small().weak());
        if self.log_files.is_empty() {
            return;
        }
        let mut pick: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("logs-folder-scroll")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("saved-log-files")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        for row in &self.log_files {
                            let selected = self.log_files_sel.as_deref() == Some(row.name.as_str());
                            if ui
                                .selectable_label(
                                    selected,
                                    egui::RichText::new(&row.name).monospace(),
                                )
                                .clicked()
                            {
                                pick = Some(row.name.clone());
                            }
                            ui.label(
                                egui::RichText::new(human_size(row.size_bytes))
                                    .small()
                                    .weak(),
                            );
                            ui.end_row();
                        }
                    });
            });
        if let Some(name) = pick {
            self.log_files_sel =
                (self.log_files_sel.as_deref() != Some(name.as_str())).then_some(name);
        }
    }
}
