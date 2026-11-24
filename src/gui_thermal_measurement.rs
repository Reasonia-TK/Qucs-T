use crate::thermal_measurement::{Measurement, MeasurementCsv};

#[derive(Default)]
pub(super) struct MeasurementView {
    file: String,
    table: Option<MeasurementCsv>,
    time_column: usize,
    temperature_column: usize,
    measurement: Option<Measurement>,
    error: String,
}

impl MeasurementView {
    pub(super) fn show(&mut self, ui: &mut egui::Ui) -> Option<&Measurement> {
        egui::CollapsingHeader::new("実測温度を重ねる").default_open(self.table.is_some()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("実測 CSV を開く…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).pick_file() {
                        match MeasurementCsv::open(&path) {
                            Ok(table) => {
                                (self.time_column, self.temperature_column) = table.default_columns();
                                self.file = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
                                self.table = Some(table);
                                self.update_measurement();
                            }
                            Err(error) => self.error = error,
                        }
                    }
                }
                if self.table.is_some() && ui.button("実測を外す").clicked() {
                    *self = Self::default();
                }
            });
            if let Some(table) = &self.table {
                ui.label(&self.file);
                let previous = (self.time_column, self.temperature_column);
                egui::Grid::new("thermal_csv_columns").num_columns(2).show(ui, |ui| {
                    choose_column(ui, "時刻列 [s]", &table.columns, &mut self.time_column);
                    choose_column(ui, "温度列 [°C]", &table.columns, &mut self.temperature_column);
                });
                if previous != (self.time_column, self.temperature_column) {
                    self.update_measurement();
                }
                if let Some(data) = &self.measurement {
                    let mut status = format!("{} 点", data.time_s.len());
                    if data.missing > 0 {
                        status.push_str(&format!("（温度が空欄の {} 行を除外）", data.missing));
                    }
                    ui.small(status);
                }
            } else {
                ui.small("UTF-8 の CSV で時刻 [s] と温度 [°C] を指定します。見出し行は省略できます。");
            }
            if !self.error.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, &self.error);
            }
        });
        self.measurement.as_ref()
    }

    fn update_measurement(&mut self) {
        let Some(csv) = &self.table else { return };
        match csv.select(self.time_column, self.temperature_column) {
            Ok(data) => {
                self.measurement = Some(data);
                self.error.clear();
            }
            Err(error) => {
                self.measurement = None;
                self.error = error;
            }
        }
    }
}

fn choose_column(ui: &mut egui::Ui, title: &str, columns: &[String], index: &mut usize) {
    ui.label(title);
    egui::ComboBox::from_id_salt(title)
        .selected_text(&columns[*index])
        .show_ui(ui, |ui| {
            for (i, name) in columns.iter().enumerate() {
                ui.selectable_value(index, i, format!("{}  {name}", i + 1));
            }
        });
    ui.end_row();
}
