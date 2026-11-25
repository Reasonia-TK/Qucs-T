//! Lumped thermal network assembled from the schematic's existing connectivity.

use crate::netlist::{Connectivity, Dsu};
use crate::schematic::{Component, Prop, Schematic};
use serde::{Deserialize, Serialize};

pub const OFFSET: f64 = 273.15;
pub const CYLINDER_AXIAL: &str = "円筒・軸方向";
pub const CYLINDER_RADIAL: &str = "円筒・半径方向";
const LEGACY_CYLINDER_RADIAL: &str = "中空円筒・半径方向";
pub const PLANAR_CONDUCTION: &str = "伝導・寸法指定";
pub const CYLINDER_LENGTH: usize = 6;
pub const CYLINDER_INNER_RADIUS: usize = 7;
pub const CYLINDER_OUTER_RADIUS: usize = 8;
pub const CONDUCTIVITY: usize = 9;
pub const CONDUCTION_LENGTH: usize = 10;
pub const CONDUCTION_AREA: usize = 11;
pub const CAPACITY_PLATE: &str = "平板";
pub const CAPACITY_CYLINDER: &str = "円筒";
pub const CAPACITY_MODE: usize = 3;
pub const CAPACITY_LENGTH: usize = 4;
pub const CAPACITY_AREA: usize = 5;
pub const CAPACITY_INNER: usize = 6;
pub const CAPACITY_OUTER: usize = 7;
pub const CAPACITY_DENSITY: usize = 8;
pub const CAPACITY_SPECIFIC_HEAT: usize = 9;
pub const BOUNDARY_TABLE: &str = "温度表";
pub const PARTS: [&str; 9] = [
    "ThResistance",
    "ThContact",
    "ThConvection",
    "ThRadiation",
    "ThCapacity",
    "ThPower",
    "ThBoundary",
    "ThCylinderAxial",
    "ThCylinderRadial",
];
const SIGMA: f64 = 5.670_374_419e-8;

pub fn is_element(typ: &str) -> bool {
    PARTS.contains(&typ)
}

pub fn is_branch(typ: &str) -> bool {
    matches!(
        typ,
        "ThResistance"
            | "ThContact"
            | "ThConvection"
            | "ThRadiation"
            | "ThCylinderAxial"
            | "ThCylinderRadial"
    )
}

pub fn normalize_model_names(typ: &str, props: &mut [Prop]) {
    let [label, model, ..] = props else { return };
    if is_branch(typ) && model.value == LEGACY_CYLINDER_RADIAL {
        model.value = CYLINDER_RADIAL.into();
        if typ == "ThCylinderRadial" && label.value == LEGACY_CYLINDER_RADIAL {
            label.value = CYLINDER_RADIAL.into();
        }
    }
}

pub fn branch_fields(component: &Component) -> Option<&'static [usize]> {
    is_branch(&component.typ).then(|| match property(component, 1) {
        "対流" => &[0, 1, 3, 4][..],
        "輻射" => &[0, 1, 4, 5][..],
        PLANAR_CONDUCTION => &[0, 1, CONDUCTIVITY, CONDUCTION_LENGTH, CONDUCTION_AREA][..],
        CYLINDER_AXIAL | CYLINDER_RADIAL => &[
            0,
            1,
            CYLINDER_LENGTH,
            CYLINDER_INNER_RADIUS,
            CYLINDER_OUTER_RADIUS,
            CONDUCTIVITY,
        ][..],
        _ => &[0, 1, 2][..],
    })
}

pub fn property_fields(component: &Component) -> Option<&'static [usize]> {
    match component.typ.as_str() {
        "ThCapacity" => Some(match property(component, CAPACITY_MODE) {
            CAPACITY_PLATE => &[0, 2, 3, 4, 5, 8, 9],
            CAPACITY_CYLINDER => &[0, 2, 3, 4, 6, 7, 8, 9],
            _ => &[0, 1, 2, 3],
        }),
        _ => branch_fields(component),
    }
}

pub fn empty() -> Schematic {
    Schematic::new_empty()
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub initial_c: f64,
    pub start_at_steady: bool,
    pub end_s: f64,
    pub output_step_s: f64,
    pub threshold_c: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            initial_c: 25.0,
            start_at_steady: false,
            end_s: 300.0,
            output_step_s: 1.0,
            threshold_c: 60.0,
        }
    }
}

impl Settings {
    pub fn read(sch: &Schematic) -> Result<Self, String> {
        match sch
            .extra_sections
            .iter()
            .find(|(name, _)| name == "ThermalAnalysis")
        {
            Some((_, lines)) => serde_json::from_str(&lines.join("\n"))
                .map_err(|e| format!("解析設定を読み込めません ({e})")),
            None => Ok(Self::default()),
        }
    }

    pub fn store(&self, sch: &mut Schematic) {
        let lines = vec![serde_json::to_string(self).expect("serializable settings")];
        if let Some((_, previous)) = sch
            .extra_sections
            .iter_mut()
            .find(|(name, _)| name == "ThermalAnalysis")
        {
            *previous = lines;
        } else {
            sch.extra_sections.push(("ThermalAnalysis".into(), lines));
        }
    }
}

#[derive(Clone, Copy)]
enum Transfer {
    Linear(f64),
    Radiation(f64),
}

impl Transfer {
    fn evaluate(self, a: f64, b: f64) -> (f64, f64, f64) {
        match self {
            Self::Linear(g) => (g * (a - b), g, g),
            Self::Radiation(k) => {
                let flow = k * (a - b) * (a + b) * (a * a + b * b);
                (flow, 4.0 * k * a.powi(3), 4.0 * k * b.powi(3))
            }
        }
    }
}

struct Branch {
    name: String,
    component: usize,
    a: usize,
    b: usize,
    transfer: Transfer,
}

struct Power {
    net: usize,
    steady: f64,
    schedule: Vec<(f64, f64)>,
}

#[derive(PartialEq)]
struct Boundary {
    steady: f64,
    schedule: Vec<(f64, f64)>,
}

impl Boundary {
    fn temperature(&self, time: Option<f64>) -> f64 {
        let Some(t) = time.filter(|_| !self.schedule.is_empty()) else {
            return self.steady;
        };
        let right = self.schedule.partition_point(|&(at, _)| at <= t);
        let (last_t, last_temp) = self.schedule[right - 1];
        if let Some(&(next_t, next_temp)) = self.schedule.get(right) {
            last_temp + (next_temp - last_temp) * ((t - last_t) / (next_t - last_t))
        } else {
            last_temp
        }
    }

    fn slope(&self, time: f64, after: bool) -> f64 {
        let right = self
            .schedule
            .partition_point(|&(at, _)| at < time || (after && at == time));
        if right == 0 || right >= self.schedule.len() {
            return 0.0;
        }
        let (a, ta) = self.schedule[right - 1];
        let (b, tb) = self.schedule[right];
        (tb - ta) / (b - a)
    }
}

#[derive(Default)]
struct Node {
    capacity: f64,
    initial: Option<f64>,
    fixed: Option<Boundary>,
}

pub struct Network {
    names: Vec<String>,
    nodes: Vec<Node>,
    branches: Vec<Branch>,
    sources: Vec<Power>,
    active: Vec<bool>,
}

#[derive(Clone, Serialize)]
pub struct Series {
    pub name: String,
    pub index: usize,
    pub values: Vec<f64>,
}

#[derive(Clone, Serialize)]
pub struct Results {
    pub time_s: Vec<f64>,
    pub temperatures_c: Vec<Series>,
    pub heat_flow_w: Vec<Series>,
    pub boundary_heat_w: Vec<Series>,
    pub balance_w: Vec<Series>,
    pub peak_temperature_c: f64,
    pub peak_release_w: f64,
}

impl Results {
    fn flow_series(&self) -> impl Iterator<Item = &Series> {
        self.heat_flow_w
            .iter()
            .chain(&self.boundary_heat_w)
            .chain(&self.balance_w)
    }

    pub fn peak(series: &Series) -> f64 {
        series
            .values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn csv(&self) -> String {
        use std::fmt::Write as _;
        let mut csv = "\u{feff}time_s".to_string();
        for (series, unit) in self
            .temperatures_c
            .iter()
            .map(|s| (s, "°C"))
            .chain(self.flow_series().map(|s| (s, "W")))
        {
            let _ = write!(csv, ",\"{} [{}]\"", series.name.replace('"', "\"\""), unit);
        }
        csv.push('\n');
        for (i, time) in self.time_s.iter().enumerate() {
            let _ = write!(csv, "{time}");
            for series in self.temperatures_c.iter().chain(self.flow_series()) {
                let _ = write!(csv, ",{}", series.values[i]);
            }
            csv.push('\n');
        }
        csv
    }

    pub fn first_reach(&self, series: &Series, target: f64) -> Option<f64> {
        let i = series.values.iter().position(|&value| value >= target)?;
        if i == 0 || self.time_s[i - 1] == self.time_s[i] {
            return Some(self.time_s[i]);
        }
        let previous = series.values[i - 1];
        let fraction = (target - previous) / (series.values[i] - previous);
        Some(self.time_s[i - 1] + fraction * (self.time_s[i] - self.time_s[i - 1]))
    }
}

pub fn property(component: &Component, index: usize) -> &str {
    component
        .props
        .get(index)
        .map(|p| p.value.as_str())
        .unwrap_or("")
}

fn finite(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("有限の数値を入力してください ({text})"))
}

fn kelvin(celsius: f64) -> Result<f64, String> {
    if !celsius.is_finite() || celsius <= -OFFSET {
        return Err("温度は −273.15 °C より高くしてください".into());
    }
    Ok(celsius + OFFSET)
}

fn positive(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label}は正の有限値にしてください"));
    }
    Ok(value)
}

pub fn resistance_from_dimensions(component: &Component) -> Option<Result<f64, String>> {
    match property(component, 1) {
        PLANAR_CONDUCTION => Some(planar_resistance(component)),
        CYLINDER_AXIAL | CYLINDER_RADIAL => Some(cylinder_resistance(component)),
        _ => None,
    }
}

fn conductivity(component: &Component) -> Result<f64, String> {
    positive(finite(property(component, CONDUCTIVITY))?, "熱伝導率")
}

fn planar_resistance(component: &Component) -> Result<f64, String> {
    let length = positive(finite(property(component, CONDUCTION_LENGTH))?, "伝熱距離")? * 1e-3;
    let area = positive(finite(property(component, CONDUCTION_AREA))?, "断面積")? * 1e-6;
    positive(length / (conductivity(component)? * area), "熱抵抗")
}

fn cylinder_resistance(component: &Component) -> Result<f64, String> {
    use std::f64::consts::{PI, TAU};

    // The drawing dimensions are in millimetres, while conductivity uses metres.
    let mm = |index| finite(property(component, index)).map(|value| value * 1e-3);
    let length = positive(mm(CYLINDER_LENGTH)?, "軸長")?;
    let inner = mm(CYLINDER_INNER_RADIUS)?;
    let outer = positive(mm(CYLINDER_OUTER_RADIUS)?, "外半径")?;
    let conductivity = conductivity(component)?;
    if inner < 0.0 || inner >= outer {
        return Err("内半径は 0 以上、外半径より小さくしてください".into());
    }
    let thickness = outer - inner;
    let resistance = match property(component, 1) {
        CYLINDER_AXIAL => length / (conductivity * PI * thickness * (outer + inner)),
        CYLINDER_RADIAL => {
            if inner == 0.0 {
                return Err("半径方向の伝導には 0 より大きい内半径が必要です".into());
            }
            let ratio = thickness / inner;
            let log_ratio = if ratio.is_finite() {
                ratio.ln_1p()
            } else {
                outer.ln() - inner.ln()
            };
            log_ratio / (TAU * conductivity * length)
        }
        _ => return Err("円筒の計算モデルを選択してください".into()),
    };
    positive(resistance, "円筒の熱抵抗")
}

pub fn capacity_from_dimensions(component: &Component) -> Option<Result<f64, String>> {
    match property(component, CAPACITY_MODE) {
        CAPACITY_PLATE | CAPACITY_CYLINDER => Some(geometry_capacity(component)),
        _ => None,
    }
}

fn geometry_capacity(component: &Component) -> Result<f64, String> {
    let num = |index| finite(property(component, index));
    let length = positive(num(CAPACITY_LENGTH)?, "長さ")? * 1e-3;
    let area = if property(component, CAPACITY_MODE) == CAPACITY_PLATE {
        positive(num(CAPACITY_AREA)?, "断面積")? * 1e-6
    } else {
        let inner = num(CAPACITY_INNER)? * 1e-3;
        let outer = positive(num(CAPACITY_OUTER)?, "外半径")? * 1e-3;
        if inner < 0.0 || inner >= outer {
            return Err("内半径は 0 以上、外半径より小さくしてください".into());
        }
        std::f64::consts::PI * (outer - inner) * (outer + inner)
    };
    let density = positive(num(CAPACITY_DENSITY)?, "密度")?;
    let specific_heat = positive(num(CAPACITY_SPECIFIC_HEAT)?, "比熱")?;
    positive(area * length * density * specific_heat, "熱容量")
}

pub(crate) fn schedule(text: &str, quantity: &str) -> Result<Vec<(f64, f64)>, String> {
    let mut points = Vec::new();
    for row in text.split(';').filter(|row| !row.trim().is_empty()) {
        let (time, value) = row.split_once(',').ok_or_else(|| {
            format!("各行に時刻 [s] と{quantity}をカンマ区切りで入力してください")
        })?;
        let (time, value) = (finite(time)?, finite(value)?);
        if points.last().map_or(time != 0.0, |&(last, _)| time <= last) {
            return Err("時系列は 0 秒から始め、時刻を昇順にしてください".into());
        }
        points.push((time, value));
    }
    if points.is_empty() {
        return Err("時系列に少なくとも 0 秒の行を入力してください".into());
    }
    Ok(points)
}

pub(crate) fn temperature_schedule(text: &str) -> Result<Vec<(f64, f64)>, String> {
    let mut temperatures = schedule(text, "温度 [°C]")?;
    for (_, temp) in &mut temperatures {
        *temp = kelvin(*temp)?;
    }
    for segment in temperatures.windows(2) {
        let [(a, ta), (b, tb)] = segment else {
            unreachable!()
        };
        if !((tb - ta) / (b - a)).is_finite() {
            return Err("温度表の変化率が大きすぎます".into());
        }
    }
    Ok(temperatures)
}

impl Network {
    pub fn build(sch: &Schematic, conn: &Connectivity) -> Result<Self, String> {
        let mut network = Self {
            names: conn.nets.iter().map(|n| n.name.clone()).collect(),
            nodes: (0..conn.nets.len()).map(|_| Node::default()).collect(),
            branches: Vec::new(),
            sources: Vec::new(),
            active: vec![false; conn.nets.len()],
        };
        let mut pins = vec![Vec::new(); sch.components.len()];
        for pin in &conn.pins {
            pins[pin.comp].push(pin.net);
        }
        for (i, c) in sch
            .components
            .iter()
            .enumerate()
            .filter(|(_, c)| c.active & 3 != 0)
        {
            network
                .add(c, i, &pins[i])
                .map_err(|e| format!("{} ({})\n{e}", c.name, property(c, 0)))?;
        }
        if !network.active.contains(&true) {
            return Err("熱回路素子を配置してください".into());
        }
        Ok(network)
    }

    fn add(&mut self, c: &Component, i: usize, pins: &[usize]) -> Result<(), String> {
        if !is_element(&c.typ) {
            return Err("熱回路で使える部品を配置してください".into());
        }
        let [a, rest @ ..] = pins else {
            return Err("端子を読み込めません".into());
        };
        let a = *a;
        self.active[a] = true;
        let prop = |index| property(c, index);
        let num = |index| finite(prop(index));
        if is_branch(&c.typ) {
            let [b] = rest else {
                return Err("２端子が必要です".into());
            };
            let b = *b;
            self.active[b] = true;
            let transfer = if let Some(resistance) = resistance_from_dimensions(c) {
                Transfer::Linear(positive(1.0 / resistance?, "熱コンダクタンス")?)
            } else {
                match prop(1) {
                    "伝導" | "接触" => Transfer::Linear(positive(
                        1.0 / positive(num(2)?, "熱抵抗")?,
                        "熱コンダクタンス",
                    )?),
                    "対流" => Transfer::Linear(positive(
                        positive(num(3)?, "熱伝達率")? * positive(num(4)?, "面積")?,
                        "熱コンダクタンス",
                    )?),
                    "輻射" => {
                        let emissivity = num(5)?;
                        if !(0.0 < emissivity && emissivity <= 1.0) {
                            return Err("有効放射率は 0 より大きく 1 以下にしてください".into());
                        }
                        Transfer::Radiation(positive(
                            SIGMA * emissivity * positive(num(4)?, "交換面積")?,
                            "輻射係数",
                        )?)
                    }
                    _ => return Err("計算モデルを選択してください".into()),
                }
            };
            self.branches.push(Branch {
                a,
                b,
                transfer,
                component: i,
                name: format!("{} {}", c.name, prop(0)),
            });
            return Ok(());
        }
        let node = &mut self.nodes[a];
        match c.typ.as_str() {
            "ThCapacity" => {
                let capacity = match capacity_from_dimensions(c) {
                    Some(result) => result?,
                    None => positive(num(1)?, "熱容量")?,
                };
                node.capacity = positive(node.capacity + capacity, "ノードの熱容量合計")?;
                if !prop(2).trim().is_empty() {
                    let initial = kelvin(num(2)?)?;
                    if node.initial.is_some_and(|other| other != initial) {
                        return Err("同じノードの初期温度をそろえてください".into());
                    }
                    node.initial = Some(initial);
                }
            }
            "ThBoundary" => {
                let mut boundary = Boundary {
                    steady: kelvin(num(1)?)?,
                    schedule: Vec::new(),
                };
                if prop(2) == BOUNDARY_TABLE {
                    boundary.schedule = temperature_schedule(prop(3))?;
                }
                if node.fixed.as_ref().is_some_and(|other| other != &boundary) {
                    return Err("同じノードに異なる境界温度が設定されています".into());
                }
                node.fixed = Some(boundary);
            }
            "ThPower" => {
                let steady = num(1)?;
                let schedule = match prop(2) {
                    "一定" => vec![(0.0, steady)],
                    "発熱表" => schedule(prop(3), "発熱量 [W]")?,
                    _ => return Err("発熱入力の種類を選択してください".into()),
                };
                self.sources.push(Power {
                    net: a,
                    steady,
                    schedule,
                });
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn check_paths(&self, steady: bool) -> Result<(), String> {
        let mut groups = Dsu::new(self.nodes.len());
        for branch in &self.branches {
            groups.union(branch.a, branch.b);
        }
        let mut anchored = vec![false; self.nodes.len()];
        for (i, node) in self.nodes.iter().enumerate() {
            if node.fixed.is_some() || (!steady && node.capacity > 0.0) {
                anchored[groups.find(i)] = true;
            }
        }
        let missing: Vec<_> = self
            .active
            .iter()
            .enumerate()
            .filter(|&(i, active)| *active && !anchored[groups.find(i)])
            .map(|(i, _)| self.names[i].as_str())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            let requirement = if steady {
                "固定温度境界への経路"
            } else {
                "固定温度境界または熱容量への経路"
            };
            Err(format!("{requirement}がありません\n{}", missing.join("、")))
        }
    }

    fn power(&self, time: Option<f64>) -> Vec<f64> {
        let mut power = vec![0.0; self.nodes.len()];
        for source in &self.sources {
            let value = time.map_or(source.steady, |t| {
                source.schedule[source.schedule.partition_point(|&(at, _)| at <= t) - 1].1
            });
            power[source.net] += value;
        }
        power
    }

    fn guess(&self, initial: f64) -> Vec<f64> {
        self.nodes
            .iter()
            .map(|n| n.initial.unwrap_or(initial))
            .collect()
    }

    pub fn steady(&self) -> Result<Results, String> {
        self.check_paths(true)?;
        let power = self.power(None);
        let temp = self.solve(&self.guess(OFFSET + 25.0), &power, Solve::Steady, None)?;
        let mut output = self.results();
        self.sample(&mut output, None, &temp, &power, false);
        Ok(output)
    }

    pub fn transient(&self, settings: &Settings) -> Result<Results, String> {
        self.check_paths(false)?;
        let end = positive(settings.end_s, "終了時刻")?;
        let step = positive(settings.output_step_s, "出力間隔")?;
        let count = (end / step).ceil();
        if count > 100_000.0 {
            return Err("出力が 100,000 区間を超えます。出力間隔を広げてください".into());
        }
        let mut temp = if settings.start_at_steady {
            self.check_paths(true)?;
            self.solve(
                &self.guess(OFFSET + 25.0),
                &self.power(None),
                Solve::Steady,
                None,
            )?
        } else {
            self.guess(kelvin(settings.initial_c)?)
        };
        let mut events: Vec<_> = self
            .sources
            .iter()
            .flat_map(|s| s.schedule.iter().map(|&(t, _)| t))
            .chain(
                self.nodes
                    .iter()
                    .filter_map(|n| n.fixed.as_ref())
                    .flat_map(|b| b.schedule.iter().map(|&(t, _)| t)),
            )
            .filter(|&t| t > 0.0 && t <= end)
            .collect();
        events.sort_by(f64::total_cmp);
        events.dedup();
        let mut stops: Vec<_> = (1..count as usize)
            .map(|i| i as f64 * step)
            .filter(|&t| t < end)
            .collect();
        stops.extend(&events);
        stops.push(end);
        stops.sort_by(f64::total_cmp);
        stops.dedup();
        let mut output = self.results();
        let mut time = 0.0;
        let mut suggested = step.min(end);
        let mut power = self.power(Some(0.0));
        temp = self.solve(&temp, &power, Solve::Frozen, Some(0.0))?;
        self.sample(&mut output, Some(time), &temp, &power, true);
        for stop in stops {
            while time < stop {
                let dt = suggested.min(stop - time);
                if time + dt <= time || dt <= 0.0 {
                    return Err(format!("{time:.6} s 付近で過渡解析が収束しません"));
                }
                let next_time = if dt >= stop - time { stop } else { time + dt };
                let trial = self.adaptive_step(&temp, &power, time, next_time);
                let Ok((next, error)) = trial else {
                    suggested = dt * 0.25;
                    continue;
                };
                suggested = dt * (0.9 / error.sqrt()).clamp(0.2, 4.0);
                if error <= 1.0 {
                    temp = next;
                    time = next_time;
                    if time < stop {
                        self.sample(&mut output, Some(time), &temp, &power, false);
                    }
                    if output.time_s.len() >= 250_000 {
                        return Err("過渡解析のステップ数が上限に達しました".into());
                    }
                }
            }
            self.sample(&mut output, Some(time), &temp, &power, false);
            if events.binary_search_by(|t| t.total_cmp(&stop)).is_ok() {
                power = self.power(Some(stop));
                temp = self.solve(&temp, &power, Solve::Frozen, Some(time))?;
                self.sample(&mut output, Some(time), &temp, &power, true);
            }
        }
        Ok(output)
    }

    fn adaptive_step(
        &self,
        old: &[f64],
        power: &[f64],
        time: f64,
        next_time: f64,
    ) -> Result<(Vec<f64>, f64), String> {
        let dt = next_time - time;
        let whole = self.solve(old, power, Solve::Step(dt), Some(next_time))?;
        let half = self.solve(old, power, Solve::Step(dt / 2.0), Some(time + dt / 2.0))?;
        let next = self.solve(&half, power, Solve::Step(dt / 2.0), Some(next_time))?;
        let error = whole
            .iter()
            .zip(&next)
            .zip(old)
            .map(|((&coarse, &fine), &initial)| {
                (coarse - fine).abs() / (1e-4 + 1e-4 * (fine - initial).abs())
            })
            .fold(0.0_f64, f64::max);
        Ok((next, error))
    }

    fn results(&self) -> Results {
        Results {
            time_s: Vec::new(),
            temperatures_c: self
                .names
                .iter()
                .enumerate()
                .filter(|(i, _)| self.active[*i])
                .map(|(index, name)| Series {
                    name: name.clone(),
                    index,
                    values: Vec::new(),
                })
                .collect(),
            heat_flow_w: self
                .branches
                .iter()
                .map(|b| Series {
                    name: b.name.clone(),
                    index: b.component,
                    values: Vec::new(),
                })
                .collect(),
            boundary_heat_w: self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.fixed.is_some())
                .map(|(index, _)| Series {
                    name: format!("境界 {}", self.names[index]),
                    index,
                    values: Vec::new(),
                })
                .collect(),
            balance_w: [
                "発熱源合計",
                "境界から加熱",
                "境界へ放熱",
                "蓄熱速度",
                "熱収支の差分",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, name)| Series {
                name: name.into(),
                index,
                values: Vec::new(),
            })
            .collect(),
            peak_temperature_c: f64::NEG_INFINITY,
            peak_release_w: 0.0,
        }
    }

    fn sample(
        &self,
        result: &mut Results,
        time: Option<f64>,
        temp: &[f64],
        power: &[f64],
        after: bool,
    ) {
        result.time_s.push(time.unwrap_or(0.0));
        for series in &mut result.temperatures_c {
            let celsius = temp[series.index] - OFFSET;
            series.values.push(celsius);
            result.peak_temperature_c = result.peak_temperature_c.max(celsius);
        }
        let mut net = power.to_vec();
        for (series, branch) in result.heat_flow_w.iter_mut().zip(&self.branches) {
            let flow = branch.transfer.evaluate(temp[branch.a], temp[branch.b]).0;
            series.values.push(flow);
            net[branch.a] -= flow;
            net[branch.b] += flow;
        }
        let mut stored = 0.0;
        let mut released = 0.0;
        let mut supplied = 0.0;
        for series in &mut result.boundary_heat_w {
            let node = &self.nodes[series.index];
            let storage = if let (Some(t), Some(boundary)) = (time, &node.fixed) {
                node.capacity * boundary.slope(t, after)
            } else {
                0.0
            };
            stored += storage;
            let exported = net[series.index] - storage;
            series.values.push(exported);
            released += exported.max(0.0);
            supplied += (-exported).max(0.0);
        }
        if time.is_some() {
            stored += self
                .nodes
                .iter()
                .zip(net)
                .filter(|(n, _)| n.fixed.is_none() && n.capacity > 0.0)
                .map(|(_, rate)| rate)
                .sum::<f64>();
        }
        let generated = power.iter().sum::<f64>();
        result.peak_release_w = result.peak_release_w.max(released);
        let residual = generated + supplied - released - stored;
        for (series, value) in result
            .balance_w
            .iter_mut()
            .zip([generated, supplied, released, stored, residual])
        {
            series.values.push(value);
        }
    }

    fn solve(
        &self,
        old: &[f64],
        power: &[f64],
        mode: Solve,
        time: Option<f64>,
    ) -> Result<Vec<f64>, String> {
        let mut temp = old.to_vec();
        let mut index = vec![None; self.nodes.len()];
        let mut free = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if let Some(boundary) = &node.fixed {
                temp[i] = boundary.temperature(time);
            } else if self.active[i] && !(matches!(mode, Solve::Frozen) && node.capacity > 0.0) {
                index[i] = Some(free.len());
                free.push(i);
            }
        }
        for _ in 0..64 {
            let (residual, matrix, scale) =
                self.balance(&temp, old, power, mode, &free, &index, true);
            let norm = scaled_norm(&residual, &scale);
            if norm <= 1.0 {
                return Ok(temp);
            }
            let delta = linear_solve(matrix, residual.into_iter().map(|v| -v).collect())?;
            let mut damping = 1.0;
            let mut improved = false;
            for _ in 0..28 {
                let mut next = temp.clone();
                for (&node, &change) in free.iter().zip(&delta) {
                    next[node] += damping * change;
                }
                if free.iter().all(|&i| next[i] > 0.0 && next[i].is_finite()) {
                    let (residual, _, _) =
                        self.balance(&next, old, power, mode, &free, &index, false);
                    if scaled_norm(&residual, &scale) < norm {
                        temp = next;
                        improved = true;
                        break;
                    }
                }
                damping *= 0.5;
            }
            if !improved {
                break;
            }
        }
        Err("熱収支が収束しません。発熱量、境界温度、素子値を確認してください".into())
    }

    fn balance(
        &self,
        temp: &[f64],
        old: &[f64],
        power: &[f64],
        mode: Solve,
        free: &[usize],
        index: &[Option<usize>],
        jacobian: bool,
    ) -> (Vec<f64>, Vec<Vec<f64>>, Vec<f64>) {
        let mut residual = Vec::with_capacity(free.len());
        let mut scale = Vec::with_capacity(free.len());
        let mut matrix = if jacobian {
            vec![vec![0.0; free.len()]; free.len()]
        } else {
            Vec::new()
        };
        for (row, &i) in free.iter().enumerate() {
            let storage = match mode {
                Solve::Step(dt) => self.nodes[i].capacity / dt,
                _ => 0.0,
            };
            let stored = storage * (temp[i] - old[i]);
            residual.push(stored - power[i]);
            scale.push(
                1e-11
                    + 1e-9 * (stored.abs() + power[i].abs())
                    + 32.0 * f64::EPSILON * storage * temp[i],
            );
            if jacobian {
                matrix[row][row] = storage;
            }
        }
        for b in self.branches.iter().filter(|b| b.a != b.b) {
            let (q, ga, gb) = b.transfer.evaluate(temp[b.a], temp[b.b]);
            for (from, to, flow, own, other) in [(b.a, b.b, q, ga, gb), (b.b, b.a, -q, gb, ga)] {
                if let Some(row) = index[from] {
                    residual[row] += flow;
                    scale[row] += 1e-9 * flow.abs()
                        + 32.0 * f64::EPSILON * (own * temp[from] + other * temp[to]);
                    if jacobian {
                        matrix[row][row] += own;
                        if let Some(col) = index[to] {
                            matrix[row][col] -= other;
                        }
                    }
                }
            }
        }
        (residual, matrix, scale)
    }
}

#[derive(Clone, Copy)]
enum Solve {
    Steady,
    Frozen,
    Step(f64),
}

fn scaled_norm(values: &[f64], scale: &[f64]) -> f64 {
    values
        .iter()
        .zip(scale)
        .map(|(v, s)| {
            if v.is_finite() && s.is_finite() {
                (v / s).abs()
            } else {
                f64::INFINITY
            }
        })
        .fold(0.0, f64::max)
}

fn linear_solve(mut matrix: Vec<Vec<f64>>, mut rhs: Vec<f64>) -> Result<Vec<f64>, String> {
    let n = rhs.len();
    for i in 0..n {
        let pivot = (i..n)
            .max_by(|&a, &b| matrix[a][i].abs().total_cmp(&matrix[b][i].abs()))
            .unwrap();
        matrix.swap(i, pivot);
        rhs.swap(i, pivot);
        let diag = matrix[i][i];
        if diag == 0.0 || !diag.is_finite() {
            return Err("熱回路の方程式を解けません".into());
        }
        for j in i + 1..n {
            let factor = matrix[j][i] / diag;
            if factor != 0.0 {
                for k in i + 1..n {
                    matrix[j][k] -= factor * matrix[i][k];
                }
                rhs[j] -= factor * rhs[i];
            }
        }
    }
    for i in (0..n).rev() {
        let contribution: f64 = matrix[i][i + 1..]
            .iter()
            .zip(&rhs[i + 1..])
            .map(|(a, b)| a * b)
            .sum();
        rhs[i] = (rhs[i] - contribution) / matrix[i][i];
        if !rhs[i].is_finite() {
            return Err("熱回路の方程式を解けません".into());
        }
    }
    Ok(rhs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{new_component, place};
    use crate::schematic::Wire;

    fn add(sch: &mut Schematic, typ: &str, name: &str, x: i32, y: i32) -> usize {
        sch.components.push(new_component(typ, name, x, y).unwrap());
        sch.components.len() - 1
    }

    fn build(sch: &Schematic) -> Network {
        Network::build(sch, &Connectivity::build(sch)).unwrap()
    }

    fn near(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} should be within {tolerance} of {expected}"
        );
    }

    #[test]
    fn geometric_capacity_matches_volume_and_drives_the_transient() {
        let mut sch = empty();
        let capacity = add(&mut sch, "ThCapacity", "C1", 0, 30);
        add(&mut sch, "ThPower", "P1", 0, 30);
        let c = &mut sch.components[capacity];
        c.props[CAPACITY_MODE].value = CAPACITY_PLATE.into();
        near(capacity_from_dimensions(c).unwrap().unwrap(), 2.43, 1e-12);
        let result = build(&sch)
            .transient(&Settings {
                end_s: 1.0,
                ..Settings::default()
            })
            .unwrap();
        near(
            *result.temperatures_c[0].values.last().unwrap(),
            25.0 + 5.0 / 2.43,
            1e-7,
        );
        near(*result.balance_w[3].values.last().unwrap(), 5.0, 1e-12);
        near(*result.balance_w[4].values.last().unwrap(), 0.0, 1e-12);
    }

    #[test]
    fn ramped_boundary_clamps_temperature_and_accounts_for_its_storage() {
        let mut sch = empty();
        add(&mut sch, "ThResistance", "R1", 0, 0);
        let hot = add(&mut sch, "ThBoundary", "Hot", -30, 30);
        add(&mut sch, "ThBoundary", "Cold", 30, 30);
        add(&mut sch, "ThCapacity", "C1", -30, 30);
        sch.components[hot].props[1].value = "40".into();
        sch.components[hot].props[2].value = BOUNDARY_TABLE.into();
        sch.components[hot].props[3].value = "0,25;2,45".into();
        let network = build(&Schematic::parse(&sch.serialize()).unwrap());
        let steady = network.steady().unwrap();
        near(steady.heat_flow_w[0].values[0], 7.5, 1e-9);
        near(steady.boundary_heat_w[0].values[0], -7.5, 1e-9);
        near(steady.boundary_heat_w[1].values[0], 7.5, 1e-9);
        let transient = network
            .transient(&Settings {
                end_s: 3.0,
                ..Settings::default()
            })
            .unwrap();
        let at = |t| {
            transient
                .time_s
                .iter()
                .position(|&value| value == t)
                .unwrap()
        };
        near(transient.temperatures_c[0].values[at(1.0)], 35.0, 1e-9);
        near(transient.boundary_heat_w[0].values[at(1.0)], -105.0, 1e-9);
        near(transient.boundary_heat_w[1].values[at(1.0)], 5.0, 1e-9);
        near(transient.balance_w[3].values[at(1.0)], 100.0, 1e-9);
        near(transient.boundary_heat_w[0].values[at(2.0)], -110.0, 1e-9);
        near(
            transient.boundary_heat_w[0].values[at(2.0) + 1],
            -10.0,
            1e-9,
        );
        near(transient.temperatures_c[0].values[at(3.0)], 45.0, 1e-9);
        assert!(transient.balance_w[4]
            .values
            .iter()
            .all(|value| value.abs() < 1e-8));
        assert!(transient
            .csv()
            .lines()
            .next()
            .unwrap()
            .contains("熱収支の差分"));
    }

    #[test]
    fn legacy_radial_model_loads_with_the_new_name_and_keeps_custom_labels() {
        let mut component = new_component("ThCylinderRadial", "Rrad1", 0, 0).unwrap();
        let resistance = cylinder_resistance(&component).unwrap();
        component.props[1].value = LEGACY_CYLINDER_RADIAL.into();
        for (label, expected) in [
            (LEGACY_CYLINDER_RADIAL, CYLINDER_RADIAL),
            ("断熱材", "断熱材"),
        ] {
            component.props[0].value = label.into();
            let line = component.serialize();
            let loaded = Component::parse(&line[1..line.len() - 1]).unwrap();
            assert_eq!(property(&loaded, 0), expected);
            assert_eq!(property(&loaded, 1), CYLINDER_RADIAL);
            near(cylinder_resistance(&loaded).unwrap(), resistance, 1e-12);
        }
    }

    #[test]
    fn planar_conduction_switches_from_direct_resistance_and_preserves_both_inputs() {
        let mut sch = empty();
        let branch = add(&mut sch, "ThResistance", "R1", 0, 0);
        add(&mut sch, "ThPower", "P1", -30, 30);
        add(&mut sch, "ThBoundary", "T1", 30, 30);
        sch.components[branch].props.truncate(6);
        let steady = |sch: &Schematic| build(sch).steady().unwrap().temperatures_c[0].values[0];
        near(steady(&sch), 35.0, 1e-8);

        let component = &mut sch.components[branch];
        let info = crate::library::lib().info(&component.typ).unwrap();
        crate::library::fill_missing_properties(component, info);
        component.props[1].value = PLANAR_CONDUCTION.into();
        component.props[CONDUCTION_LENGTH].value = "10".into();
        component.props[CONDUCTION_AREA].value = "250".into();
        component.props[CONDUCTIVITY].value = "200".into();
        near(planar_resistance(component).unwrap(), 0.2, 1e-12);
        for (index, value) in [
            (CONDUCTION_LENGTH, "-1"),
            (CONDUCTION_AREA, "0"),
            (CONDUCTIVITY, "NaN"),
        ] {
            let mut invalid = component.clone();
            invalid.props[index].value = value.into();
            assert!(planar_resistance(&invalid).is_err());
        }

        let mut reloaded = Schematic::parse(&sch.serialize()).unwrap();
        near(steady(&reloaded), 26.0, 1e-8);
        let transient = build(&reloaded)
            .transient(&Settings {
                end_s: 1.0,
                ..Settings::default()
            })
            .unwrap();
        near(transient.temperatures_c[0].values[0], 26.0, 1e-8);
        reloaded.components[branch].props[1].value = "伝導".into();
        near(steady(&reloaded), 35.0, 1e-8);
        assert_eq!(
            property(&reloaded.components[branch], CONDUCTION_AREA),
            "250"
        );
    }

    #[test]
    fn cylindrical_resistance_uses_mm_and_preserves_values_when_saved() {
        let mut sch = empty();
        let branch = add(&mut sch, "ThCylinderAxial", "Rax1", 0, 0);
        add(&mut sch, "ThPower", "P1", -30, 30);
        add(&mut sch, "ThBoundary", "T1", 30, 30);
        let pi = std::f64::consts::PI;
        let solid = 0.1 / (15.0 * pi * 0.01_f64.powi(2));
        near(
            cylinder_resistance(&sch.components[branch]).unwrap(),
            solid,
            1e-12,
        );

        sch.components[branch].props[CYLINDER_INNER_RADIUS].value = "5".into();
        let hollow = 0.1 / (15.0 * pi * (0.01_f64.powi(2) - 0.005_f64.powi(2)));
        near(
            cylinder_resistance(&sch.components[branch]).unwrap(),
            hollow,
            1e-12,
        );
        let result = build(&sch).steady().unwrap();
        near(
            result.temperatures_c[0].values[0],
            25.0 + 5.0 * hollow,
            1e-7,
        );

        let mut radial = new_component("ThCylinderRadial", "Rrad1", 0, 0).unwrap();
        radial.props[0].value = "断熱材".into();
        radial.props[CONDUCTIVITY].value = "0.04".into();
        sch.components[branch] = radial;
        let saved = Schematic::parse(&sch.serialize()).unwrap();
        assert_eq!(sch, saved);
        let resistance = 2.0_f64.ln() / (2.0 * pi * 0.04 * 0.1);
        near(
            cylinder_resistance(&saved.components[branch]).unwrap(),
            resistance,
            1e-12,
        );
        let network = build(&saved);
        let result = network.steady().unwrap();
        near(
            result.temperatures_c[0].values[0],
            25.0 + 5.0 * resistance,
            1e-7,
        );
        near(result.heat_flow_w[0].values[0], 5.0, 1e-7);
        let transient = network
            .transient(&Settings {
                end_s: 1.0,
                ..Settings::default()
            })
            .unwrap();
        near(transient.heat_flow_w[0].values[0], 5.0, 1e-7);
    }

    #[test]
    fn cylindrical_models_validate_geometry_and_can_extend_a_legacy_branch() {
        let sch = Schematic::parse(include_str!("../examples/heatsink.qucst")).unwrap();
        let mut component = sch
            .components
            .into_iter()
            .find(|c| c.typ == "ThContact")
            .unwrap();
        let original = component.props.clone();
        let info = crate::library::lib().info(&component.typ).unwrap();
        crate::library::fill_missing_properties(&mut component, info);
        assert_eq!(&component.props[..original.len()], &original);
        component.props[1].value = CYLINDER_RADIAL.into();
        assert!(cylinder_resistance(&component).is_ok());

        for (index, value) in [
            (CYLINDER_INNER_RADIUS, "0"),
            (CYLINDER_INNER_RADIUS, "-1"),
            (CYLINDER_INNER_RADIUS, "10"),
            (CYLINDER_INNER_RADIUS, "20"),
            (CONDUCTIVITY, "NaN"),
            (CONDUCTIVITY, "0"),
            (CYLINDER_LENGTH, "-1"),
        ] {
            let mut invalid = component.clone();
            invalid.props[index].value = value.into();
            assert!(cylinder_resistance(&invalid).is_err(), "{index} = {value}");
        }
    }

    #[test]
    fn shared_symbol_properties_and_single_rc_match_closed_form() {
        let mut sch = empty();
        let r = add(&mut sch, PARTS[0], "R1", 0, 0);
        let c = add(&mut sch, PARTS[4], "C1", -30, 30);
        add(&mut sch, PARTS[5], "P1", -90, 30);
        add(&mut sch, PARTS[6], "T1", 30, 30);
        sch.wires.push(Wire::new(-90, 0, -30, 0));
        sch.components[r].props[0].value = "別の \"表示名\"".into();
        assert_eq!(Schematic::parse(&sch.serialize()).unwrap(), sch);
        let pins = place(&sch.components[r]).pins;
        for typ in &PARTS[1..4] {
            let other = place(&new_component(typ, "X", 0, 0).unwrap());
            assert_eq!(
                other.pins.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
                pins.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
            );
        }
        let net = Connectivity::build(&sch);
        let node = net.pins.iter().find(|pin| pin.comp == c).unwrap().net;
        let network = build(&sch);
        let steady = network.steady().unwrap();
        near(
            steady
                .temperatures_c
                .iter()
                .find(|s| s.index == node)
                .unwrap()
                .values[0],
            35.0,
            1e-8,
        );
        let transient = network
            .transient(&Settings {
                end_s: 20.0,
                ..Settings::default()
            })
            .unwrap();
        let series = transient
            .temperatures_c
            .iter()
            .find(|s| s.index == node)
            .unwrap();
        near(
            *series.values.last().unwrap(),
            25.0 + 10.0 * (1.0 - (-1.0_f64).exp()),
            0.03,
        );
        near(
            transient.first_reach(series, 30.0).unwrap(),
            20.0 * 2.0_f64.ln(),
            0.1,
        );
    }

    #[test]
    fn sample_balances_convection_and_radiation_and_cools_when_power_stops() {
        let sch = Schematic::parse(include_str!("../examples/heatsink.qucst")).unwrap();
        let saved = Schematic::parse(&sch.serialize()).unwrap();
        assert_eq!(sch, saved);
        let network = build(&saved);
        let steady = network.steady().unwrap();
        let fluid = steady
            .heat_flow_w
            .iter()
            .find(|s| s.name.starts_with("H1"))
            .unwrap()
            .values[0];
        let radiation = steady
            .heat_flow_w
            .iter()
            .find(|s| s.name.starts_with("Rad1"))
            .unwrap()
            .values[0];
        let sink = steady
            .temperatures_c
            .iter()
            .find(|s| s.name == "ヒートシンク")
            .unwrap()
            .values[0];
        near(fluid, 8.0 * 0.08 * (sink - 25.0), 1e-8);
        near(
            radiation,
            SIGMA * 0.85 * 0.08 * ((sink + OFFSET).powi(4) - (22.0 + OFFSET).powi(4)),
            1e-8,
        );
        near(fluid + radiation, 20.0, 1e-7);
        let result = network.transient(&Settings::read(&saved).unwrap()).unwrap();
        let part = result
            .temperatures_c
            .iter()
            .find(|s| s.name == "発熱部品")
            .unwrap();
        let at = |time| part.values[result.time_s.iter().position(|&t| t == time).unwrap()];
        assert!(at(100.0) > at(10.0));
        assert!(at(150.0) < at(100.0));
        assert!(at(240.0) > at(150.0));
        assert!(result
            .csv()
            .lines()
            .next()
            .unwrap()
            .contains("Rad1 輻射 [W]"));
    }

    #[test]
    fn floating_capacity_is_allowed_and_massless_node_updates_at_the_event() {
        let mut sch = empty();
        add(&mut sch, PARTS[0], "R1", 0, 0);
        add(&mut sch, PARTS[4], "C1", 30, 30);
        let p = add(&mut sch, PARTS[5], "P1", -30, 30);
        sch.components[p].props[2].value = "発熱表".into();
        sch.components[p].props[3].value = "0,0;0.3,5;1,0".into();
        let network = build(&sch);
        assert!(network.steady().is_err());
        let result = network
            .transient(&Settings {
                end_s: 2.0,
                ..Settings::default()
            })
            .unwrap();
        let first = result.time_s.iter().position(|&t| t == 0.3).unwrap();
        assert_eq!(result.time_s[first + 1], 0.3);
        let flow = &result.heat_flow_w[0].values;
        near(flow[first], 0.0, 1e-8);
        near(flow[first + 1], 5.0, 1e-8);
        near(
            *result.temperatures_c[1].values.last().unwrap(),
            25.35,
            1e-7,
        );
        assert!(network
            .transient(&Settings {
                start_at_steady: true,
                ..Settings::default()
            })
            .is_err());
    }
}
