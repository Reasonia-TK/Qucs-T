use super::*;
use crate::thermal::{self, Network, Results, Series, Settings};
use crate::thermal_grid::TemperatureMap;
#[path = "gui_thermal_cylinder.rs"]
mod cylinder_ui;
pub(super) use cylinder_ui::CylinderDialog;
#[path = "gui_thermal_map.rs"]
mod map_ui;
#[path = "gui_thermal_measurement.rs"]
mod measurement_ui;
use measurement_ui::MeasurementView;
use std::sync::Arc;

#[derive(Default, Clone, Copy, PartialEq)]
enum SignalGroup {
    #[default]
    Temperature,
    Branch,
    Boundary,
    Balance,
}

impl SignalGroup {
    fn series(self, result: &Results) -> &[Series] {
        match self {
            Self::Temperature => &result.temperatures_c,
            Self::Branch => &result.heat_flow_w,
            Self::Boundary => &result.boundary_heat_w,
            Self::Balance => &result.balance_w,
        }
    }

    fn unit(self) -> &'static str {
        if self == Self::Temperature {
            "°C"
        } else {
            "W"
        }
    }
}

pub(super) struct Comparison {
    steady: bool,
    title: String,
    results: Arc<Results>,
    map: Option<TemperatureMap>,
}

#[derive(Default)]
pub(super) struct ThermalView {
    pub open: bool,
    settings: Settings,
    steady: bool,
    group: SignalGroup,
    series: usize,
    comparison_series: Option<usize>,
    map: Option<TemperatureMap>,
    map_open: bool,
    map_time: f64,
    map_point: Option<(f64, f64)>,
    measured: MeasurementView,
    error: String,
    result: Option<(bool, Schematic, Arc<Results>)>,
    pending: Option<(bool, Schematic, Receiver<Result<Results, String>>)>,
}

impl ThermalView {
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

impl App {
    pub(super) fn thermal_palette(&mut self, ui: &mut egui::Ui) {
        ui.heading("熱回路素子");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for typ in thermal::PARTS {
                if let Some(part) = lib().info(typ) {
                    self.palette_entry(
                        ui,
                        typ,
                        &part.caption,
                        &part.description,
                        &part.description,
                    );
                }
            }
        });
    }

    pub(super) fn open_thermal(&mut self) {
        if !self.thermal.open {
            match Settings::read(&self.doc.sch) {
                Ok(settings) => self.thermal.settings = settings,
                Err(error) => self.thermal.error = error,
            }
        }
        self.thermal.open = true;
    }

    pub(super) fn open_thermal_sample(&mut self) {
        match Schematic::parse(include_str!("../examples/heatsink.qucst")) {
            Ok(sch) => {
                self.new_tab(Doc::new(sch, None));
                self.status = "サンプルを開きました".into();
            }
            Err(error) => self.status = error,
        }
    }

    pub(super) fn thermal_run(&mut self, steady: bool, ctx: &egui::Context) {
        if self.thermal.pending.is_some() {
            return;
        }
        self.open_thermal();
        self.apply_thermal_settings();
        self.thermal.steady = steady;
        self.thermal.error.clear();
        self.thermal.result = None;
        self.thermal.map = None;
        let schematic = self.doc.sch.clone();
        let input = schematic.clone();
        let settings = self.thermal.settings.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.thermal.pending = Some((steady, schematic, receiver));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let connection = Connectivity::build(&input);
            let output = Network::build(&input, &connection).and_then(|network| {
                if steady {
                    network.steady()
                } else {
                    network.transient(&settings)
                }
            });
            let _ = sender.send(output);
            ctx.request_repaint();
        });
    }

    fn apply_thermal_settings(&mut self) {
        let mut changed = self.finish_buffered_property_edit();
        if Settings::read(&self.doc.sch).ok().as_ref() != Some(&self.thermal.settings) {
            self.doc.checkpoint();
            self.thermal.settings.store(&mut self.doc.sch);
            changed = true;
        }
        if changed {
            self.changed();
        }
    }

    pub(super) fn thermal_panel(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        if let Some((_, _, receiver)) = &self.thermal.pending {
            match receiver.try_recv() {
                Ok(output) => {
                    let (steady, snapshot, _) = self.thermal.pending.take().unwrap();
                    match output {
                        Ok(results) => {
                            self.thermal.map = TemperatureMap::new(
                                &snapshot,
                                &Connectivity::build(&snapshot),
                                &results,
                            );
                            self.thermal.map_time = 0.0;
                            self.thermal.result = Some((steady, snapshot, Arc::new(results)));
                        }
                        Err(error) => self.thermal.error = error,
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.thermal.pending = None;
                    self.thermal.error = "解析処理が終了しました".into();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut open = self.thermal.open;
        if !open {
            return;
        }
        let mut action = None;
        let mut apply = false;
        let mut set_baseline = false;
        let mut clear_baseline = false;
        let accent = self.pal().accent;
        let reference_color = self.pal().muted;
        let measured_color = if ctx.theme() == egui::Theme::Dark {
            egui::Color32::from_rgb(108, 199, 229)
        } else {
            egui::Color32::from_rgb(26, 97, 129)
        };
        let baseline = self.thermal_comparison.as_ref();
        egui::SidePanel::right("thermal_analysis")
            .default_width(420.0)
            .width_range(300.0..=(ctx.content_rect().width() * 0.55).max(300.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("熱解析");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("閉じる").clicked() {
                            open = false;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let view = &mut self.thermal;
                    let busy = view.pending.is_some();
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("定常解析"))
                            .clicked()
                        {
                            action = Some(true);
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("過渡解析"))
                            .clicked()
                        {
                            action = Some(false);
                        }
                        if busy {
                            ui.spinner();
                            ui.label("計算中");
                        }
                    });
                    egui::CollapsingHeader::new("過渡解析の設定")
                        .default_open(view.result.is_none())
                        .show(ui, |ui| {
                            egui::Grid::new("thermal_settings")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    ui.label("開始温度");
                                    ui.horizontal(|ui| {
                                        ui.selectable_value(
                                            &mut view.settings.start_at_steady,
                                            false,
                                            "初期温度",
                                        );
                                        ui.selectable_value(
                                            &mut view.settings.start_at_steady,
                                            true,
                                            "定常解",
                                        );
                                    });
                                    ui.end_row();
                                    ui.label("共通初期温度 [°C]");
                                    ui.add_enabled(
                                        !view.settings.start_at_steady,
                                        egui::DragValue::new(&mut view.settings.initial_c)
                                            .range(-273.149..=1e6),
                                    );
                                    ui.end_row();
                                    for (label, value) in [
                                        ("終了時刻 [s]", &mut view.settings.end_s),
                                        ("最大出力間隔 [s]", &mut view.settings.output_step_s),
                                        ("到達判定温度 [°C]", &mut view.settings.threshold_c),
                                    ] {
                                        ui.label(label);
                                        let range = if label.ends_with("[s]") {
                                            1e-9..=1e12
                                        } else {
                                            -273.149..=1e6
                                        };
                                        ui.add(egui::DragValue::new(value).speed(0.1).range(range));
                                        ui.end_row();
                                    }
                                });
                            apply = ui.button("設定を回路図に適用").clicked();
                        });
                    if !view.error.is_empty() {
                        ui.colored_label(ui.visuals().error_fg_color, &view.error);
                    }
                    let Some((steady, snapshot, results)) = &view.result else {
                        return;
                    };
                    ui.separator();
                    if snapshot != &self.doc.sch {
                        ui.label("回路図が変更されました。再解析してください。");
                        return;
                    }
                    ui.horizontal_wrapped(|ui| {
                        if view.map.is_some() && ui.button("温度マップ…").clicked() {
                            view.map_open = true;
                        }
                        set_baseline = ui.button("この結果を比較基準にする").clicked();
                        if baseline.is_some() {
                            clear_baseline = ui.button("基準を解除").clicked();
                        }
                    });
                    let comparable = baseline.filter(|b| b.steady == *steady);
                    if let Some(reference) = comparable {
                        comparison_summary(ui, results, reference);
                    } else if let Some(reference) = baseline {
                        ui.small(format!(
                            "比較基準 {} と同じ解析を実行すると比較できます。",
                            reference.title
                        ));
                    }
                    if *steady {
                        ui.heading("定常結果");
                        ui.small("枝の熱流は端子 1 → 端子 2 が正");
                        egui::Grid::new("thermal_steady")
                            .striped(true)
                            .show(ui, |ui| {
                                for (series, unit) in results
                                    .temperatures_c
                                    .iter()
                                    .map(|s| (s, "°C"))
                                    .chain(results.heat_flow_w.iter().map(|s| (s, "W")))
                                {
                                    ui.label(&series.name);
                                    ui.monospace(format!("{:.3} {unit}", series.values[0]));
                                    ui.end_row();
                                }
                            });
                        steady_flow_table(ui, "境界への熱流", &results.boundary_heat_w);
                        steady_flow_table(ui, "熱収支", &results.balance_w);
                    } else {
                        ui.heading("過渡結果");
                        ui.horizontal_wrapped(|ui| {
                            for (group, label) in [
                                (SignalGroup::Temperature, "温度"),
                                (SignalGroup::Branch, "枝の熱流"),
                                (SignalGroup::Boundary, "境界の熱流"),
                                (SignalGroup::Balance, "熱収支"),
                            ] {
                                if ui.selectable_value(&mut view.group, group, label).changed() {
                                    view.series = 0;
                                    view.comparison_series = None;
                                }
                            }
                        });
                        let group = view.group.series(results);
                        if !group.is_empty() {
                            view.series = view.series.min(group.len() - 1);
                            let old_series = view.series;
                            choose_series(ui, "thermal_series", "表示", group, &mut view.series);
                            if view.series != old_series {
                                view.comparison_series = None;
                            }
                            let series = &group[view.series];
                            let unit = view.group.unit();
                            let overlay = comparable.and_then(|reference| {
                                let references = view.group.series(&reference.results);
                                if references.is_empty() {
                                    return None;
                                }
                                let automatic = references
                                    .iter()
                                    .position(|s| s.name == series.name)
                                    .unwrap_or(0);
                                let mut selected = view
                                    .comparison_series
                                    .filter(|&i| i < references.len())
                                    .unwrap_or(automatic);
                                let previous = selected;
                                choose_series(
                                    ui,
                                    "thermal_baseline_series",
                                    "基準",
                                    references,
                                    &mut selected,
                                );
                                if selected != previous {
                                    view.comparison_series = Some(selected);
                                }
                                Some(PlotTrace {
                                    time: &reference.results.time_s,
                                    series: &references[selected],
                                    color: reference_color,
                                    kind: TraceKind::Reference,
                                })
                            });
                            let measured = if view.group == SignalGroup::Temperature {
                                view.measured.show(ui).map(|data| PlotTrace {
                                    time: &data.time_s,
                                    series: &data.temperature,
                                    color: measured_color,
                                    kind: TraceKind::Measured,
                                })
                            } else {
                                None
                            };
                            if let Some(trace) = &measured {
                                if trace.time[trace.time.len() - 1] < results.time_s[0]
                                    || trace.time[0] > results.time_s[results.time_s.len() - 1]
                                {
                                    ui.small("解析と実測の時刻範囲が重なっていません。");
                                }
                            }
                            if overlay.is_some() || measured.is_some() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.colored_label(accent, "実線 現在");
                                    if overlay.is_some() {
                                        ui.colored_label(reference_color, "破線 基準");
                                    }
                                    if let Some(trace) = &measured {
                                        ui.colored_label(
                                            trace.color,
                                            format!("● 実測 {}", trace.series.name),
                                        );
                                    }
                                });
                            }
                            let traces = overlay
                                .into_iter()
                                .chain([PlotTrace {
                                    time: &results.time_s,
                                    series,
                                    color: accent,
                                    kind: TraceKind::Current,
                                }])
                                .chain(measured)
                                .collect::<Vec<_>>();
                            plot(ui, &traces, unit);
                            if view.group == SignalGroup::Temperature {
                                let peak = Results::peak(series);
                                let threshold = view.settings.threshold_c;
                                let reach = results
                                    .first_reach(series, threshold)
                                    .map_or("未到達".into(), |t| format!("約 {t:.3} s"));
                                ui.label(format!(
                                "最高温度 {peak:.3} °C    {threshold:.2} °C への初回到達 {reach}"
                            ));
                            }
                            if view.group == SignalGroup::Boundary {
                                ui.small("境界へ流れる熱を正、境界から入る熱を負で表示します。");
                            }
                        } else {
                            ui.label("この回路に表示できる信号がありません。");
                        }
                    }
                    if ui.button("CSV を保存…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("CSV", &["csv"])
                            .set_file_name("thermal-results.csv")
                            .save_file()
                        {
                            if let Err(error) = std::fs::write(path, results.csv()) {
                                view.error = format!("CSV を保存できません ({error})");
                            }
                        }
                    }
                });
            });
        self.thermal.open = open;
        if clear_baseline {
            self.thermal_comparison = None;
        }
        if set_baseline {
            if let Some((steady, _, results)) = &self.thermal.result {
                let title = self
                    .doc
                    .path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("無題の回路 {}", self.tab_id));
                self.thermal_comparison = Some(Comparison {
                    steady: *steady,
                    title,
                    results: Arc::clone(results),
                    map: self.thermal.map.clone(),
                });
                self.thermal.comparison_series = None;
            }
        }
        if apply {
            self.apply_thermal_settings();
        }
        if let Some(steady) = action {
            self.thermal_run(steady, ctx);
        }
    }

    pub(super) fn paint_thermal(&self, painter: &egui::Painter, canvas: Rect) {
        let Some((true, snapshot, result)) = &self.thermal.result else {
            return;
        };
        if snapshot != &self.doc.sch {
            return;
        }
        let pal = self.pal();
        let mut occupied: Vec<_> = self
            .scene
            .shapes
            .iter()
            .filter_map(|shape| {
                let Shape::Text {
                    pos: (x, y),
                    text,
                    size,
                    ..
                } = shape
                else {
                    return None;
                };
                let (width, height) = library::text_size(text, *size);
                Some(
                    Rect::from_two_pos(
                        self.to_screen(canvas, *x, *y),
                        self.to_screen(canvas, x + width, y + height),
                    )
                    .expand(4.0),
                )
            })
            .collect();
        occupied.extend(self.conn.placed.iter().map(|placed| {
            let [x1, y1, x2, y2] = placed.bbox;
            Rect::from_two_pos(
                self.to_screen(canvas, x1, y1),
                self.to_screen(canvas, x2, y2),
            )
            .expand(3.0)
        }));
        let mut label = |x, y, text| {
            let anchor = self.to_screen(canvas, x, y);
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), pal.on_accent);
            let size = galley.size() + Vec2::new(8.0, 2.0);
            let x = size.x * 0.5;
            let y = size.y * 0.5;
            let candidates = [
                Vec2::new(0.0, -y - 8.0),
                Vec2::new(0.0, y + 8.0),
                Vec2::new(x + 8.0, -y - 8.0),
                Vec2::new(-x - 8.0, -y - 8.0),
                Vec2::new(x + 8.0, y + 8.0),
                Vec2::new(-x - 8.0, y + 8.0),
                Vec2::new(0.0, -3.0 * y - 12.0),
                Vec2::new(0.0, 3.0 * y + 12.0),
                Vec2::new(0.0, -5.0 * y - 16.0),
                Vec2::new(0.0, 5.0 * y + 16.0),
                Vec2::new(x + 12.0, -3.0 * y - 12.0),
                Vec2::new(-x - 12.0, -3.0 * y - 12.0),
            ];
            let extra = [56.0, 82.0, 108.0]
                .into_iter()
                .flat_map(|distance| [Vec2::new(0.0, -distance), Vec2::new(0.0, distance)]);
            let Some(bounds) = candidates
                .into_iter()
                .chain(extra)
                .map(|offset| Rect::from_center_size(anchor + offset, size))
                .find(|bounds| {
                    canvas.contains_rect(*bounds)
                        && !occupied.iter().any(|other| bounds.intersects(*other))
                })
            else {
                return;
            };
            painter.line_segment(
                [anchor, bounds.clamp(anchor)],
                Stroke::new(1.0_f32, pal.accent),
            );
            painter.rect_filled(bounds, 3.0, pal.accent);
            painter.galley(bounds.min + Vec2::new(4.0, 1.0), galley, pal.on_accent);
            occupied.push(bounds.expand(3.0));
        };
        for series in &result.temperatures_c {
            let net = &self.conn.nets[series.index];
            let at = net
                .wires
                .iter()
                .map(|&i| &self.doc.sch.wires[i])
                .max_by_key(|w| w.len())
                .map(|w| {
                    (
                        (f64::from(w.x1) + f64::from(w.x2)) * 0.5,
                        (f64::from(w.y1) + f64::from(w.y2)) * 0.5,
                    )
                })
                .or_else(|| {
                    net.pins.first().map(|&i| {
                        (
                            f64::from(self.conn.pins[i].x),
                            f64::from(self.conn.pins[i].y),
                        )
                    })
                });
            if let Some((x, y)) = at {
                label(x, y, format!("{:.2} °C", series.values[0]));
            }
        }
        for series in &result.heat_flow_w {
            let component = &self.doc.sch.components[series.index];
            label(
                f64::from(component.x),
                f64::from(component.y),
                format!("{:+.3} W", series.values[0]),
            );
        }
    }
}

fn choose_series(ui: &mut egui::Ui, id: &str, label: &str, group: &[Series], index: &mut usize) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(&group[*index].name)
            .show_ui(ui, |ui| {
                for (i, series) in group.iter().enumerate() {
                    ui.selectable_value(index, i, &series.name);
                }
            });
    });
}

fn steady_flow_table(ui: &mut egui::Ui, heading: &str, series: &[Series]) {
    egui::CollapsingHeader::new(heading).show(ui, |ui| {
        if series.is_empty() {
            ui.label("境界がありません。");
        }
        egui::Grid::new(("steady_flow", heading))
            .striped(true)
            .show(ui, |ui| {
                for item in series {
                    ui.label(&item.name);
                    ui.monospace(format!("{:.4} W", item.values[0]));
                    ui.end_row();
                }
            });
    });
}

fn comparison_summary(ui: &mut egui::Ui, current: &Results, reference: &Comparison) {
    egui::CollapsingHeader::new(format!("比較基準 {}", reference.title))
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("thermal_comparison")
                .striped(true)
                .show(ui, |ui| {
                    for title in ["指標", "現在", "基準", "差"] {
                        ui.strong(title);
                    }
                    ui.end_row();
                    for (title, now, then) in [
                        (
                            "最高温度 [°C]",
                            current.peak_temperature_c,
                            reference.results.peak_temperature_c,
                        ),
                        (
                            "最大放熱 [W]",
                            current.peak_release_w,
                            reference.results.peak_release_w,
                        ),
                    ] {
                        ui.label(title);
                        ui.monospace(format!("{now:.3}"));
                        ui.monospace(format!("{then:.3}"));
                        ui.monospace(format!("{:+.3}", now - then));
                        ui.end_row();
                    }
                });
        });
}

#[derive(Clone, Copy, PartialEq)]
enum TraceKind {
    Current,
    Reference,
    Measured,
}

impl TraceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Current => "現在",
            Self::Reference => "基準",
            Self::Measured => "実測",
        }
    }
}

struct PlotTrace<'a> {
    time: &'a [f64],
    series: &'a Series,
    color: egui::Color32,
    kind: TraceKind,
}

fn plot(ui: &mut egui::Ui, traces: &[PlotTrace<'_>], unit: &str) {
    let (outer, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(180.0), 235.0),
        Sense::hover(),
    );
    let rect = Rect::from_min_max(
        outer.min + egui::vec2(62.0, 18.0),
        outer.max - egui::vec2(14.0, 32.0),
    );
    let painter = ui.painter_at(outer);
    let text = ui.visuals().text_color();
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let values = || {
        traces
            .iter()
            .flat_map(|trace| trace.series.values.iter().copied())
    };
    let low = values().fold(f64::INFINITY, f64::min);
    let high = values().fold(f64::NEG_INFINITY, f64::max);
    let margin = if high == low {
        0.5
    } else {
        (high - low) * 0.07
    };
    let start = traces
        .iter()
        .map(|trace| trace.time[0])
        .fold(f64::INFINITY, f64::min);
    let end = traces
        .iter()
        .map(|trace| trace.time[trace.time.len() - 1])
        .fold(f64::NEG_INFINITY, f64::max);
    let point = |t: f64, y: f64| {
        Pos2::new(
            egui::lerp(rect.x_range(), ((t - start) / (end - start)) as f32),
            egui::lerp(
                rect.y_range(),
                ((high + margin - y) / (high - low + 2.0 * margin)) as f32,
            ),
        )
    };
    let font = FontId::proportional(12.0);
    for i in 0..=4 {
        let fraction = i as f32 / 4.0;
        let y = egui::lerp(rect.y_range(), fraction);
        painter.hline(rect.x_range(), y, stroke);
        painter.text(
            Pos2::new(rect.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            format!(
                "{:.2}",
                high + margin - (high - low + 2.0 * margin) * f64::from(fraction)
            ),
            font.clone(),
            text,
        );
        let x = egui::lerp(rect.x_range(), fraction);
        painter.vline(x, rect.y_range(), stroke);
        painter.text(
            Pos2::new(x, rect.bottom() + 4.0),
            Align2::CENTER_TOP,
            format!("{:.3}", start + (end - start) * f64::from(fraction)),
            font.clone(),
            text,
        );
    }
    painter.text(outer.left_top(), Align2::LEFT_TOP, unit, font.clone(), text);
    painter.text(
        outer.center_bottom(),
        Align2::CENTER_BOTTOM,
        "時刻 [s]",
        font,
        text,
    );
    for trace in traces {
        let points = curve_points(trace.time, trace.series, rect.width(), point);
        match trace.kind {
            TraceKind::Reference => painter.extend(egui::Shape::dashed_line(
                &points,
                Stroke::new(1.5_f32, trace.color),
                5.0,
                3.0,
            )),
            TraceKind::Current => {
                painter.add(egui::Shape::line(points, Stroke::new(1.7_f32, trace.color)));
            }
            TraceKind::Measured => {
                for &pos in &points {
                    painter.circle_filled(pos, 2.3_f32, trace.color);
                }
                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points,
                        Stroke::new(0.8_f32, trace.color.gamma_multiply(0.65)),
                    ));
                }
            }
        }
    }
    if let Some(at) = response.hover_pos().filter(|at| rect.contains(*at)) {
        let hit_t = start + (end - start) * f64::from((at.x - rect.left()) / rect.width());
        painter.vline(at.x, rect.y_range(), stroke);
        let mut tooltip = Vec::new();
        let calculation = traces.iter().find(|trace| trace.kind == TraceKind::Current);
        for trace in traces {
            let time = trace.time;
            let index = nearest_sample(time, hit_t);
            let value = trace.series.values[index];
            let position = point(time[index], value);
            if !(time[0]..=time[time.len() - 1]).contains(&hit_t)
                && (trace.kind != TraceKind::Measured || (at.x - position.x).abs() > 8.0)
            {
                continue;
            }
            painter.circle_filled(position, 4.0, trace.color);
            tooltip.push(format!(
                "{} {:.4} s  {:.4} {unit}",
                trace.kind.label(),
                time[index],
                value
            ));
            if trace.kind == TraceKind::Measured {
                if let Some(estimated) = calculation.and_then(|line| {
                    crate::plot::sample_at(line.time, &line.series.values, time[index])
                }) {
                    tooltip.push(format!(
                        "実測時刻の差（解析 − 実測） {:+.4} {unit}",
                        estimated - value
                    ));
                }
            }
        }
        response.on_hover_text(tooltip.join("\n"));
    }
}

fn nearest_sample(time: &[f64], t: f64) -> usize {
    let right = time.partition_point(|&at| at <= t).min(time.len() - 1);
    let left = right.saturating_sub(1);
    if (t - time[left]).abs() <= (time[right] - t).abs() {
        left
    } else {
        right
    }
}

fn curve_points(
    time: &[f64],
    series: &Series,
    width: f32,
    point: impl Fn(f64, f64) -> Pos2,
) -> Vec<Pos2> {
    let mut points = Vec::new();
    // Keep each bucket's extrema in chronological order so short peaks and steps survive.
    let chunk_size = time.len().div_ceil((width as usize).max(1)).max(1);
    for (chunk, values) in series.values.chunks(chunk_size).enumerate() {
        let extreme = |reversed: bool| {
            values
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    if reversed {
                        b.total_cmp(a)
                    } else {
                        a.total_cmp(b)
                    }
                })
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        let mut indices = [0, extreme(false), extreme(true), values.len() - 1];
        indices.sort_unstable();
        let mut previous = None;
        for index in indices {
            if previous == Some(index) {
                continue;
            }
            previous = Some(index);
            let i = chunk * chunk_size + index;
            points.push(point(time[i], values[index]));
        }
    }
    points
}
