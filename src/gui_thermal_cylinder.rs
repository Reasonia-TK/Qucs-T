use super::*;
use crate::thermal_grid::{
    self, CylinderGrid, Layer, MaterialLibrary, Profile, Surface, SurfaceKind, Sweep,
    SweepParameter, SweepRow, MAX_CELLS, SURFACE_NAMES,
};

type SweepOutput = Result<Vec<SweepRow>, String>;

#[derive(Default)]
pub(in crate::gui) struct CylinderDialog {
    open: bool,
    grid: CylinderGrid,
    layer: usize,
    materials: MaterialLibrary,
    sweep: Sweep,
    running: Option<Receiver<SweepOutput>>,
    rows: Vec<SweepRow>,
    last_sweep: Option<Sweep>,
    error: String,
}

impl App {
    pub(in crate::gui) fn open_cylinder_window(&mut self) {
        let dialog = &mut self.cylinder_dialog;
        if let Some(grid) = CylinderGrid::read(&self.doc.sch) {
            dialog.grid = grid;
            dialog.layer = 0;
            dialog.sweep.observe_z = dialog.grid.length_mm * 0.5;
            dialog.sweep.observe_r = (dialog.grid.inner_mm + dialog.grid.layers[0].outer_mm) * 0.5;
        }
        match thermal_grid::load_materials() {
            Ok(materials) => {
                dialog.materials = materials;
                dialog.error.clear();
            }
            Err(error) => dialog.error = error,
        }
        dialog.open = true;
    }

    pub(in crate::gui) fn cylinder_window(&mut self, ctx: &egui::Context) {
        let dialog = &mut self.cylinder_dialog;
        if let Some(receiver) = &dialog.running {
            use std::sync::mpsc::TryRecvError;
            match receiver.try_recv() {
                Ok(result) => {
                    dialog.running = None;
                    match result {
                        Ok(rows) => dialog.rows = rows,
                        Err(error) => dialog.error = error,
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    dialog.running = None;
                    dialog.error = "スイープが終了しました".into();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if !dialog.open {
            return;
        }

        let mut open = true;
        let mut generate = false;
        let mut run_sweep = false;
        egui::Window::new("円筒回路を生成")
            .open(&mut open)
            .collapsible(false)
            .default_width(510.0)
            .vscroll(true)
            .max_height((ctx.content_rect().height() - 80.0).max(230.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    generate = ui.button("新しいタブに生成").clicked();
                    if ui.button("片端加熱＋外面対流").clicked() {
                        for (surface, kind) in dialog.grid.surfaces.iter_mut().zip([
                            SurfaceKind::Power,
                            SurfaceKind::Insulated,
                            SurfaceKind::Insulated,
                            SurfaceKind::Convection,
                        ]) {
                            surface.kind = kind;
                        }
                    }
                });
                if !dialog.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, &dialog.error);
                }
                ui.separator();
                ui.heading("寸法と分割");
                egui::Grid::new("cylinder_dimensions")
                    .num_columns(2)
                    .show(ui, |ui| {
                        number(ui, "軸長 [mm]", &mut dialog.grid.length_mm);
                        number(ui, "内半径 [mm]", &mut dialog.grid.inner_mm);
                        ui.label("軸方向の分割数");
                        ui.add(egui::DragValue::new(&mut dialog.grid.axial).range(1..=MAX_CELLS));
                        ui.end_row();
                    });
                ui.small(format!(
                    "{} セル（上限 {MAX_CELLS}）。中実円筒は内半径を 0 に設定します。",
                    dialog.grid.axial * dialog.grid.radial_count()
                ));
                ui.separator();
                dialog.layers(ui);
                ui.separator();
                egui::CollapsingHeader::new("表面条件")
                    .default_open(true)
                    .show(ui, |ui| {
                        for (i, (surface, label)) in dialog
                            .grid
                            .surfaces
                            .iter_mut()
                            .zip(SURFACE_NAMES)
                            .enumerate()
                        {
                            if i == 2 && dialog.grid.inner_mm == 0.0 {
                                continue;
                            }
                            surface_form(ui, surface, label);
                        }
                    });
                if !dialog.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, &dialog.error);
                }
                ui.separator();
                egui::CollapsingHeader::new("生成条件のスイープ").show(ui, |ui| {
                    run_sweep = dialog.sweep_panel(ui);
                });
            });
        dialog.open = open;
        if run_sweep {
            match Settings::read(&self.doc.sch) {
                Ok(settings) => dialog.start_sweep(ctx, settings),
                Err(error) => dialog.error = error,
            }
        }
        if generate {
            match dialog.grid.generate() {
                Ok(mut schematic) => {
                    if let Ok(settings) = Settings::read(&self.doc.sch) {
                        settings.store(&mut schematic);
                    }
                    let mut doc = Doc::new(schematic, None);
                    doc.dirty = true;
                    let status = format!(
                        "円筒回路を生成しました（{} × {} セル）",
                        dialog.grid.axial,
                        dialog.grid.radial_count()
                    );
                    dialog.open = false;
                    dialog.error.clear();
                    self.new_tab(doc);
                    self.status = status;
                }
                Err(error) => dialog.error = error,
            }
        }
    }
}

impl CylinderDialog {
    fn layers(&mut self, ui: &mut egui::Ui) {
        ui.heading("層と材料");
        ui.horizontal_wrapped(|ui| {
            for i in 0..self.grid.layers.len() {
                ui.selectable_value(&mut self.layer, i, format!("層 {}", i + 1));
            }
            if ui.button("外側に層を追加").clicked() {
                let last = self.grid.layers.last().expect("at least one layer");
                self.grid.layers.push(Layer {
                    outer_mm: last.outer_mm + 5.0,
                    radial: 1,
                    material: last.material.clone(),
                    contact_to_inner: 0.0,
                });
                self.layer = self.grid.layers.len() - 1;
            }
            if self.grid.layers.len() > 1 && ui.button("選択した層を除く").clicked() {
                self.grid.layers.remove(self.layer);
                self.layer = self.layer.min(self.grid.layers.len() - 1);
            }
        });
        let inner = if self.layer == 0 {
            self.grid.inner_mm
        } else {
            self.grid.layers[self.layer - 1].outer_mm
        };
        let layer = &mut self.grid.layers[self.layer];
        ui.small(format!("この層の内半径 {inner} mm"));
        egui::Grid::new(("cylinder_layer", self.layer))
            .num_columns(2)
            .show(ui, |ui| {
                number(ui, "外半径 [mm]", &mut layer.outer_mm);
                ui.label("この層の半径方向の分割数");
                ui.add(egui::DragValue::new(&mut layer.radial).range(1..=MAX_CELLS));
                ui.end_row();
                if self.layer > 0 {
                    ui.label("内側の層との接触熱抵抗 [m²·K/W]")
                        .on_hover_text("0 のときは界面の内側と外側を同じ温度として扱います。");
                    ui.add(
                        egui::DragValue::new(&mut layer.contact_to_inner)
                            .speed(0.00001)
                            .max_decimals(12),
                    );
                    ui.end_row();
                }
                ui.label("材料名");
                ui.text_edit_singleline(&mut layer.material.name);
                ui.end_row();
                number(ui, "熱伝導率 [W/(m·K)]", &mut layer.material.conductivity);
                number(ui, "密度 [kg/m³]", &mut layer.material.density);
                number(ui, "比熱 [J/(kg·K)]", &mut layer.material.specific_heat);
            });
        ui.horizontal_wrapped(|ui| {
            if !self.materials.is_empty() {
                egui::ComboBox::from_id_salt("registered_material")
                    .selected_text("登録材料を適用…")
                    .show_ui(ui, |ui| {
                        for (name, material) in &self.materials {
                            if ui.button(name).clicked() {
                                layer.material = material.clone();
                                ui.close();
                            }
                        }
                    });
            }
            if ui.button("材料を登録／更新").clicked() {
                match thermal_grid::register_material(&layer.material) {
                    Ok(materials) => {
                        self.materials = materials;
                        self.error.clear();
                    }
                    Err(error) => self.error = error,
                }
            }
        });
    }

    fn sweep_panel(&mut self, ui: &mut egui::Ui) -> bool {
        ui.small("この画面の生成条件を使います。観測位置は各試行で同じ軸位置・半径です。");
        let sweep = &mut self.sweep;
        let before = sweep.parameter;
        egui::ComboBox::from_id_salt("sweep_parameter")
            .selected_text(parameter_label(sweep.parameter))
            .show_ui(ui, |ui| {
                for (value, label) in SweepParameter::OPTIONS {
                    ui.selectable_value(&mut sweep.parameter, value, label);
                }
            });
        if sweep.parameter != before {
            (sweep.from, sweep.to, sweep.count) = match sweep.parameter {
                SweepParameter::OuterThickness => (2.0, 10.0, 5),
                SweepParameter::OuterConvection => (5.0, 25.0, 5),
                SweepParameter::Mesh => (1.0, 3.0, 3),
            };
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut sweep.steady, true, "定常");
            ui.selectable_value(&mut sweep.steady, false, "過渡");
        });
        egui::Grid::new("cylinder_sweep_values")
            .num_columns(2)
            .show(ui, |ui| {
                number(ui, "開始値", &mut sweep.from);
                number(ui, "終了値", &mut sweep.to);
                ui.label("試行数");
                ui.add(egui::DragValue::new(&mut sweep.count).range(2..=50));
                ui.end_row();
                number(ui, "観測点の軸位置 [mm]", &mut sweep.observe_z);
                number(ui, "観測点の半径 [mm]", &mut sweep.observe_r);
            });
        let run = ui
            .add_enabled(self.running.is_none(), egui::Button::new("スイープを実行"))
            .clicked();
        if self.running.is_some() {
            ui.spinner();
        }
        if let Some(spec) = self.last_sweep.as_ref().filter(|_| !self.rows.is_empty()) {
            ui.small(format!(
                "{} · {}解析",
                parameter_label(spec.parameter),
                if spec.steady { "定常" } else { "過渡" }
            ));
            ui.small(format!(
                "観測位置 z = {:.3} mm、r = {:.3} mm",
                spec.observe_z, spec.observe_r
            ));
            egui::Grid::new("sweep_rows").striped(true).show(ui, |ui| {
                for title in [
                    "設定値",
                    "セル数",
                    "最高温度 [°C]",
                    "最大放熱 [W]",
                    "観測点最高 [°C]",
                ] {
                    ui.strong(title);
                }
                ui.end_row();
                for row in &self.rows {
                    ui.label(format!("{:.4}", row.value));
                    ui.label(row.cells.to_string());
                    match &row.result {
                        Ok(result) => {
                            ui.label(format!("{:.3}", result.temperature));
                            ui.label(format!("{:.3}", result.release));
                            ui.label(result.observed.map_or("—".into(), |v| format!("{v:.3}")));
                        }
                        Err(error) => {
                            ui.colored_label(ui.visuals().error_fg_color, "エラー")
                                .on_hover_text(error);
                            ui.label("—");
                            ui.label("—");
                        }
                    }
                    ui.end_row();
                }
            });
            if ui.button("スイープ CSV を保存…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .set_file_name("thermal-sweep.csv")
                    .save_file()
                {
                    if let Err(error) = std::fs::write(path, sweep_csv(spec, &self.rows)) {
                        self.error = format!("CSV を保存できません（{error}）");
                    }
                }
            }
        }
        run
    }

    fn start_sweep(&mut self, ctx: &egui::Context, settings: Settings) {
        self.error.clear();
        self.rows.clear();
        self.last_sweep = Some(self.sweep.clone());
        let grid = self.grid.clone();
        let sweep = self.sweep.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.running = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(sweep.run(&grid, &settings));
            ctx.request_repaint();
        });
    }
}

fn number(ui: &mut egui::Ui, label: &str, value: &mut f64) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).speed(0.1));
    ui.end_row();
}

fn surface_form(ui: &mut egui::Ui, surface: &mut Surface, label: &str) {
    egui::Grid::new(("cylinder_surface", label))
        .num_columns(2)
        .show(ui, |ui| {
            ui.strong(label);
            let selected = SurfaceKind::OPTIONS
                .iter()
                .find(|&&(kind, _)| kind == surface.kind)
                .unwrap()
                .1;
            egui::ComboBox::from_id_salt(label)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (kind, name) in SurfaceKind::OPTIONS {
                        ui.selectable_value(&mut surface.kind, kind, name);
                    }
                });
            ui.end_row();
            if surface.kind == SurfaceKind::Temperature {
                number(ui, "表面温度 [°C]", &mut surface.temperature);
            }
            if surface.kind == SurfaceKind::Power {
                number(ui, "面全体の発熱量 [W]", &mut surface.power);
            }
            if surface.kind.convection() {
                number(ui, "流体温度 [°C]", &mut surface.temperature);
                number(ui, "熱伝達率 [W/(m²·K)]", &mut surface.h);
            }
            if surface.kind.radiation() {
                number(ui, "周囲温度 [°C]", &mut surface.surroundings);
                number(ui, "有効放射率", &mut surface.emissivity);
            }
        });
    ui.push_id(label, |ui| {
        if surface.kind == SurfaceKind::Power {
            profile_form(
                ui,
                &mut surface.power_profile,
                "面全体の発熱表を使う",
                "時刻 [s], 面全体の発熱量 [W]",
                surface.power,
            );
        }
        if surface.kind == SurfaceKind::Temperature || surface.kind.convection() {
            let toggle = if surface.kind.convection() {
                "流体の温度表を使う"
            } else {
                "表面の温度表を使う"
            };
            profile_form(
                ui,
                &mut surface.temperature_profile,
                toggle,
                "時刻 [s], 温度 [°C]（線形補間）",
                surface.temperature,
            );
        }
        if surface.kind.radiation() {
            profile_form(
                ui,
                &mut surface.surroundings_profile,
                "輻射先の温度表を使う",
                "時刻 [s], 周囲温度 [°C]（線形補間）",
                surface.surroundings,
            );
        }
    });
}

fn profile_form(ui: &mut egui::Ui, profile: &mut Profile, toggle: &str, label: &str, initial: f64) {
    ui.push_id(toggle, |ui| {
        if ui.checkbox(&mut profile.enabled, toggle).changed()
            && profile.enabled
            && profile.rows.trim().is_empty()
        {
            profile.rows = format!("0,{initial}");
        }
        if profile.enabled {
            ui.label(label);
            ui.add(
                egui::TextEdit::multiline(&mut profile.rows)
                    .desired_rows(4)
                    .desired_width(250.0)
                    .code_editor(),
            );
        }
    });
}

fn parameter_label(parameter: SweepParameter) -> &'static str {
    SweepParameter::OPTIONS
        .iter()
        .find(|&&(kind, _)| kind == parameter)
        .unwrap()
        .1
}

fn sweep_csv(spec: &Sweep, rows: &[SweepRow]) -> String {
    use std::fmt::Write as _;
    let mut csv =
        format!(
        "\u{feff}{},セル数,最高温度 [°C],最大放熱 [W],観測点最高 [°C] (z={} mm r={} mm),エラー\n",
        parameter_label(spec.parameter), spec.observe_z, spec.observe_r
    );
    for row in rows {
        match &row.result {
            Ok(result) => {
                let observed = result.observed.map_or(String::new(), |v| v.to_string());
                let _ = writeln!(
                    csv,
                    "{},{},{},{},{},",
                    row.value, row.cells, result.temperature, result.release, observed
                );
            }
            Err(error) => {
                let _ = writeln!(
                    csv,
                    "{},{},,,,\"{}\"",
                    row.value,
                    row.cells,
                    error.replace('"', "\"\"")
                );
            }
        }
    }
    csv
}
