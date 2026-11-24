use super::*;

impl App {
    pub(in crate::gui) fn temperature_map_window(&mut self, ctx: &egui::Context) {
        let view = &mut self.thermal;
        if !view.map_open {
            return;
        }
        let Some((steady, snapshot, results)) = &view.result else {
            return;
        };
        let Some(map) = view.map.as_ref().filter(|_| snapshot == &self.doc.sch) else {
            return;
        };
        let reference = self
            .thermal_comparison
            .as_ref()
            .filter(|r| r.steady == *steady);
        let mut show_series = None;
        egui::Window::new("円筒の温度マップ")
            .open(&mut view.map_open)
            .default_width(610.0)
            .show(ctx, |ui| {
                if !steady {
                    let end = *results.time_s.last().expect("at least one sample");
                    ui.add(
                        egui::Slider::new(&mut view.map_time, 0.0..=end)
                            .text("時刻 [s]")
                            .fixed_decimals(3),
                    );
                }
                let time = view.map_time;
                let values: Vec<_> = map
                    .signals
                    .iter()
                    .map(|signal| signal.map(|i| temperature(results, i, time)))
                    .collect();
                let (mut minimum, mut maximum) = (f64::INFINITY, f64::NEG_INFINITY);
                for value in values.iter().flatten() {
                    minimum = minimum.min(*value);
                    maximum = maximum.max(*value);
                }
                if !minimum.is_finite() {
                    ui.label("解析結果に対応する円筒セルがありません。");
                    return;
                }
                let (outer, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width().max(230.0), 230.0),
                    Sense::click(),
                );
                let rect = Rect::from_min_max(
                    outer.min + egui::vec2(75.0, 18.0),
                    outer.max - egui::vec2(20.0, 35.0),
                );
                let painter = ui.painter_at(outer);
                let thickness = map.grid.layers.last().unwrap().outer_mm - map.grid.inner_mm;
                let fraction = |r: f64| ((r - map.grid.inner_mm) / thickness) as f32;
                let loc = |z: f64, r: f64| {
                    Pos2::new(
                        egui::lerp(rect.x_range(), (z / map.grid.length_mm) as f32),
                        egui::lerp(rect.y_range(), fraction(r)),
                    )
                };
                let point_at = |pos: Pos2| {
                    (
                        (f64::from((pos.x - rect.left()) / rect.width())) * map.grid.length_mm,
                        map.grid.inner_mm
                            + f64::from((pos.y - rect.top()) / rect.height()) * thickness,
                    )
                };
                let dz = map.grid.length_mm / map.grid.axial as f64;
                let outline = Stroke::new(0.6_f32, ui.visuals().window_stroke.color);
                for (r, ring) in map.grid.rings().enumerate() {
                    for z in 0..map.grid.axial {
                        let cell = Rect::from_two_pos(
                            loc(z as f64 * dz, ring.inner),
                            loc((z + 1) as f64 * dz, ring.outer),
                        );
                        let color = values[r * map.grid.axial + z].map_or(
                            ui.visuals().faint_bg_color,
                            |value| {
                                heat_color(if maximum == minimum {
                                    0.5
                                } else {
                                    (value - minimum) / (maximum - minimum)
                                })
                            },
                        );
                        painter.rect(cell, 0.0, color, outline, egui::StrokeKind::Inside);
                    }
                }
                let position = response
                    .hover_pos()
                    .filter(|p| rect.contains(*p))
                    .map(point_at);
                if response.clicked() {
                    view.map_point = position;
                }
                let font = FontId::proportional(12.0);
                let text = ui.visuals().text_color();
                for (anchor, align, label) in [
                    (
                        rect.left_bottom() + Vec2::new(0.0, 4.0),
                        Align2::LEFT_TOP,
                        "0".into(),
                    ),
                    (
                        rect.right_bottom() + Vec2::new(0.0, 4.0),
                        Align2::RIGHT_TOP,
                        format!("{} mm", map.grid.length_mm),
                    ),
                    (
                        rect.left_top() - Vec2::new(5.0, 0.0),
                        Align2::RIGHT_CENTER,
                        format!("{} mm", map.grid.inner_mm),
                    ),
                    (
                        rect.left_bottom() - Vec2::new(5.0, 0.0),
                        Align2::RIGHT_CENTER,
                        format!("{} mm", map.grid.inner_mm + thickness),
                    ),
                    (
                        outer.center_bottom(),
                        Align2::CENTER_BOTTOM,
                        "軸位置 z".into(),
                    ),
                    (outer.left_top(), Align2::LEFT_TOP, "半径 r".into()),
                ] {
                    painter.text(anchor, align, label, font.clone(), text);
                }
                ui.horizontal(|ui| {
                    ui.colored_label(heat_color(0.0), format!("低温 {minimum:.2} °C"));
                    ui.colored_label(heat_color(1.0), format!("高温 {maximum:.2} °C"));
                    ui.small("表示時刻での範囲");
                });
                if let Some((z, r)) = view.map_point.or(position) {
                    if let Some(signal) = map.signal_at(z, r) {
                        let now = temperature(results, signal, time);
                        let at = loc(z, r);
                        painter.circle_filled(at, 4.0, egui::Color32::WHITE);
                        painter.circle_stroke(at, 4.0, Stroke::new(1.2_f32, egui::Color32::BLACK));
                        ui.label(format!(
                            "軸位置 {z:.2} mm、半径 {r:.2} mm  現在 {now:.3} °C"
                        ));
                        if let Some(reference) =
                            reference.and_then(|b| b.map.as_ref().map(|m| (b, m)))
                        {
                            let (baseline, map) = reference;
                            if let Some(signal) = map
                                .signal_at(z, r)
                                .filter(|_| time <= *baseline.results.time_s.last().unwrap())
                            {
                                let then = temperature(&baseline.results, signal, time);
                                ui.label(format!(
                                    "比較基準 {then:.3} °C  差 {:+.3} °C",
                                    now - then
                                ));
                            } else {
                                ui.label("この位置・時刻に対応する比較基準の温度がありません。");
                            }
                        }
                    }
                }
                ui.small("セルの代表温度を表示します。図をクリックすると観測位置を固定します。");
                if view.map_point.is_some() && ui.button("観測位置の固定を解除").clicked()
                {
                    view.map_point = None;
                }
                if !steady {
                    if let Some(signal) = view.map_point.and_then(|(z, r)| map.signal_at(z, r)) {
                        if ui.button("この位置の過渡波形を表示").clicked() {
                            show_series = Some(signal);
                        }
                    }
                }
            });
        if let Some(signal) = show_series {
            view.group = SignalGroup::Temperature;
            view.series = signal;
            view.comparison_series = None;
            view.open = true;
            view.map_open = false;
        }
    }
}

fn temperature(results: &Results, series: usize, time: f64) -> f64 {
    let values = &results.temperatures_c[series].values;
    crate::plot::sample_at(&results.time_s, values, time).unwrap_or(values[0])
}

fn heat_color(t: f64) -> egui::Color32 {
    let stops = [
        [49.0, 100.0, 165.0],
        [107.0, 172.0, 180.0],
        [238.0, 206.0, 119.0],
        [176.0, 63.0, 55.0],
    ];
    let at = t.clamp(0.0, 1.0) * 3.0;
    let index = (at as usize).min(2);
    let channel = |i: usize| {
        (stops[index][i] + (stops[index + 1][i] - stops[index][i]) * (at - index as f64)).round()
            as u8
    };
    egui::Color32::from_rgb(channel(0), channel(1), channel(2))
}
