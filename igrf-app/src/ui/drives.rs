use super::*;

impl IgrfApp {
    pub(crate) fn copy_drive_to_logs(&mut self) {
        let (Some(name), Some(cwd)) = (self.ext_sel.clone(), self.ext_cwd.clone()) else {
            return;
        };
        let dest = self.log_directory();
        self.copy_into(cwd.join(name), dest);
    }

    pub(crate) fn copy_logs_to_drive(&mut self) {
        let (Some(name), Some(cwd)) = (self.log_files_sel.clone(), self.ext_cwd.clone()) else {
            return;
        };
        let source = self.log_directory().join(name);
        self.copy_into(source, cwd);
    }

    /// Copy `source` into `dest_dir`, keeping its file name. Overwrites a file of
    /// the same name. Both lists are rescanned afterwards so the new file shows up.
    pub(crate) fn copy_into(&mut self, source: PathBuf, dest_dir: PathBuf) {
        let Some(name) = source.file_name().map(|name| name.to_owned()) else {
            self.transfer_status = "No file selected".to_owned();
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&dest_dir) {
            self.transfer_status = format!("Cannot open {}: {error}", dest_dir.display());
            return;
        }
        let dest = dest_dir.join(&name);
        let replaced = dest.exists();
        match std::fs::copy(&source, &dest) {
            Ok(bytes) => {
                self.transfer_status = format!(
                    "{} {} ({}) to {}",
                    if replaced { "Replaced" } else { "Copied" },
                    name.to_string_lossy(),
                    human_size(bytes),
                    dest_dir.display()
                );
            }
            Err(error) => {
                self.transfer_status = format!("Copy failed: {error}");
            }
        }
        self.refresh_log_files();
        self.refresh_ext_files();
    }

    /// Right column: pick a connected drive from the dropdown
    pub(crate) fn show_external_drive_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("Rescan")
                .on_hover_text("Look for newly connected drives")
                .clicked()
            {
                self.refresh_drives();
            }
            let selected_text = self
                .ext_drives
                .get(self.ext_drive_sel)
                .map(|drive| drive.label.clone())
                .unwrap_or_else(|| "(no drive)".to_owned());
            let mut pick: Option<usize> = None;
            egui::ComboBox::from_id_salt("ext-drive")
                .width(60.0)
                .wrap_mode(egui::TextWrapMode::Truncate)
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for (index, drive) in self.ext_drives.iter().enumerate() {
                        if ui
                            .selectable_label(index == self.ext_drive_sel, &drive.label)
                            .clicked()
                        {
                            pick = Some(index);
                        }
                    }
                });
            if let Some(index) = pick {
                self.select_ext_drive(index);
            }
        });

        if self.ext_drives.is_empty() {
            ui.label(
                egui::RichText::new("No drives detected - plug one in and Rescan")
                    .small()
                    .weak(),
            );
            return;
        }
        if self.ext_cwd.is_none() {
            ui.label(egui::RichText::new("Pick a drive above").small().weak());
            return;
        }

        ui.horizontal(|ui| {
            let at_root = self.ext_cwd.as_deref().and_then(Path::parent).is_none();
            if ui
                .add_enabled(!at_root, egui::Button::new("Back"))
                .clicked()
            {
                if let Some(parent) = self.ext_cwd.as_deref().and_then(Path::parent) {
                    self.navigate_ext(parent.to_path_buf());
                }
            }
            if ui.button("Refresh").clicked() {
                self.refresh_ext_files();
            }
        });
        ui.label(egui::RichText::new(&self.ext_status).small().weak());

        let mut into: Option<PathBuf> = None;
        let mut pick: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("ext-drive-scroll")
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("ext-drive-files")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        for row in &self.ext_entries {
                            if row.is_dir {
                                if ui
                                    .selectable_label(false, format!("[dir]  {}", row.name))
                                    .clicked()
                                {
                                    if let Some(cwd) = &self.ext_cwd {
                                        into = Some(cwd.join(&row.name));
                                    }
                                }
                                ui.label("");
                            } else {
                                let selected = self.ext_sel.as_deref() == Some(row.name.as_str());
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
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(path) = into {
            self.navigate_ext(path);
        } else if let Some(name) = pick {
            self.ext_sel = (self.ext_sel.as_deref() != Some(name.as_str())).then_some(name);
        }
    }

    /// Rebuild the connected-drive list for the dropdown, keeping the current
    /// selection pointed at the same root when it is still present.
    pub(crate) fn refresh_drives(&mut self) {
        let previous = self
            .ext_drives
            .get(self.ext_drive_sel)
            .map(|drive| drive.root.clone());
        self.ext_drives = list_drives();
        self.ext_drive_sel = previous
            .and_then(|root| self.ext_drives.iter().position(|drive| drive.root == root))
            .unwrap_or(0);
        // Drop a browse position that belonged to a drive now unplugged.
        if let Some(cwd) = self.ext_cwd.clone() {
            if !self
                .ext_drives
                .iter()
                .any(|drive| cwd.starts_with(&drive.root))
            {
                self.ext_cwd = None;
                self.ext_entries.clear();
                self.ext_status.clear();
                self.ext_sel = None;
            }
        }
    }

    /// Point the browser at a drive's root and list it.
    pub(crate) fn select_ext_drive(&mut self, index: usize) {
        self.ext_drive_sel = index;
        if let Some(drive) = self.ext_drives.get(index) {
            self.ext_cwd = Some(PathBuf::from(&drive.root));
            self.ext_sel = None;
            self.refresh_ext_files();
        }
    }

    /// Move the browser to `dir` and list it.
    pub(crate) fn navigate_ext(&mut self, dir: PathBuf) {
        self.ext_cwd = Some(dir);
        self.ext_sel = None;
        self.refresh_ext_files();
    }

    /// List the current external directory: folders first, then files by name.
    pub(crate) fn refresh_ext_files(&mut self) {
        self.ext_entries.clear();
        let Some(dir) = self.ext_cwd.clone() else {
            self.ext_status = "No drive selected".to_owned();
            return;
        };
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let Ok(meta) = entry.metadata() else { continue };
                    let is_dir = meta.is_dir();
                    if !is_dir && !meta.is_file() {
                        continue;
                    }
                    self.ext_entries.push(FileRow {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        is_dir,
                        size_bytes: if is_dir { 0 } else { meta.len() },
                    });
                }
                self.ext_entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                self.ext_status = format!("{} item(s)", self.ext_entries.len());
                prune_selection(&mut self.ext_sel, &self.ext_entries);
            }
            Err(error) => {
                self.ext_status = format!("Cannot read {}: {error}", dir.display());
            }
        }
    }
}
