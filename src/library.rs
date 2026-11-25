//! Thermal component definitions and schematic symbol geometry.

use crate::schematic::Component;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct PropInfo {
    pub name: String,
    pub default: String,
    pub display: bool,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct CompInfo {
    pub typ: String,
    pub caption: String,
    pub description: String,
    pub category: String,
    pub prefix: String,
    pub props: Vec<PropInfo>,
    pub pin_names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Pen {
    pub color: String,
    pub width: f64,
    pub style: String,
}

#[derive(Clone, Debug)]
pub enum Prim {
    Line { x1: f64, y1: f64, x2: f64, y2: f64, pen: Pen },
    Ellipse { x: f64, y: f64, w: f64, h: f64, pen: Pen, brush: Option<String> },
}

#[derive(Clone, Debug, Default)]
pub struct SymbolShape {
    pub prims: Vec<Prim>,
    pub ports: Vec<[f64; 2]>,
    pub bbox: [f64; 4],
    pub tx: f64,
    pub ty: f64,
    pub rotated: i32,
}

#[derive(Clone, Debug, Default)]
pub struct SymbolDef {
    pub shape: SymbolShape,
    pub pin_names: Vec<String>,
}

pub struct Library {
    pub comps: Vec<CompInfo>,
    by_type: HashMap<String, usize>,
    pub symbols: HashMap<String, SymbolDef>,
}

static LIB: OnceLock<Library> = OnceLock::new();

pub fn lib() -> &'static Library {
    LIB.get_or_init(|| {
        let mut comps = Vec::new();
        let mut symbols = HashMap::new();
        add_thermal_parts(&mut comps, &mut symbols);
        let by_type = comps.iter().enumerate().map(|(i, c)| (c.typ.clone(), i)).collect();
        Library { comps, by_type, symbols }
    })
}

fn add_thermal_parts(comps: &mut Vec<CompInfo>, symbols: &mut HashMap<String, SymbolDef>) {
    use crate::thermal::{CYLINDER_AXIAL, CYLINDER_RADIAL, PLANAR_CONDUCTION};

    let pen = Pen { color: "#000080".into(), width: 2.0, style: "SolidLine".into() };
    let line = |x1, y1, x2, y2| Prim::Line { x1, y1, x2, y2, pen: pen.clone() };
    let prop = |name: &str, default: &str, display, description: &str| PropInfo {
        name: name.into(), default: default.into(), display, description: description.into(),
    };
    let labels = ["伝導", "接触", "対流", "輻射", "熱容量", "発熱源", "固定温度", CYLINDER_AXIAL, CYLINDER_RADIAL];
    let descriptions = [
        "熱抵抗 [K/W]", "接触熱抵抗 [K/W]", "熱伝達率と面積", "有効放射率と交換面積",
        "直接入力または寸法・材料指定", "定常発熱量と発熱表", "一定温度または温度表",
        "丸棒・パイプの端面間", "パイプの内面と外面の間",
    ];
    let models = format!("[伝導, {PLANAR_CONDUCTION}, 接触, 対流, 輻射, {CYLINDER_AXIAL}, {CYLINDER_RADIAL}]");
    for (i, typ) in crate::thermal::PARTS.into_iter().enumerate() {
        let label = labels[i];
        let mut props = vec![prop("表示ラベル", label, true, "回路図に表示する任意のラベル")];
        let symbol = if crate::thermal::is_branch(typ) {
            props.extend([
                prop("計算モデル", label, false, &models),
                prop("R [K/W]", "2", false, "熱抵抗"),
                prop("h [W/(m²·K)]", "10", false, "熱伝達率"),
                prop("A [m²]", "0.1", false, "面積"),
                prop("有効放射率", "0.8", false, "0 より大きく 1 以下"),
                prop("軸長 [mm]", "100", false, "円筒の軸方向の長さ"),
                prop("内半径 [mm]", if typ == "ThCylinderAxial" { "0" } else { "5" }, false, "中実の円筒では 0、半径方向の伝導では正の値"),
                prop("外半径 [mm]", "10", false, "内半径より大きい値"),
                prop("熱伝導率 [W/(m·K)]", "15", false, "材料の一定熱伝導率"),
                prop("伝熱距離 [mm]", "10", false, "板の厚さなど、熱が流れる方向の距離"),
                prop("断面積 [mm²]", "100", false, "熱流に垂直な断面積"),
            ]);
            // Retain the Qucs-S rectangular resistor geometry for all thermal branches.
            SymbolDef {
                shape: SymbolShape {
                    prims: vec![
                        line(-18.0, -9.0, 18.0, -9.0),
                        line(18.0, -9.0, 18.0, 9.0),
                        line(18.0, 9.0, -18.0, 9.0),
                        line(-18.0, 9.0, -18.0, -9.0),
                        line(-30.0, 0.0, -18.0, 0.0),
                        line(18.0, 0.0, 30.0, 0.0),
                    ],
                    ports: vec![[-30.0, 0.0], [30.0, 0.0]],
                    bbox: [-30.0, -11.0, 30.0, 11.0],
                    tx: -26.0,
                    ty: 15.0,
                    rotated: 0,
                },
                pin_names: vec!["1".into(), "2".into()],
            }
        } else {
            let mut prims = vec![line(0.0, -30.0, 0.0, -10.0)];
            match typ {
                "ThCapacity" => {
                    props.extend([
                        prop("C [J/K]", "10", false, "ノードに加える熱容量"),
                        prop("初期温度 [°C]", "", false, "空欄の場合は共通初期温度を使用"),
                        prop("容量設定", "直接入力", false, "[直接入力, 平板, 円筒]"),
                        prop("厚さ・軸長 [mm]", "10", false, "平板の厚さまたは円筒の軸長"),
                        prop("断面積 [mm²]", "100", false, "厚さ方向に垂直な断面積"),
                        prop("内半径 [mm]", "5", false, "中実円筒では 0"),
                        prop("外半径 [mm]", "10", false, "内半径より大きい値"),
                        prop("密度 [kg/m³]", "2700", false, "材料の密度"),
                        prop("比熱 [J/(kg·K)]", "900", false, "材料の一定比熱"),
                    ]);
                    prims.extend([line(-12.0, -10.0, 12.0, -10.0), line(-12.0, -3.0, 12.0, -3.0)]);
                }
                "ThPower" => {
                    props.extend([
                        prop("定常発熱 [W]", "5", false, "定常解析の発熱量。一定モードでは過渡解析でも使用"),
                        prop("発熱入力", "一定", false, "[一定, 発熱表]"),
                        prop("発熱表", "0,0;10,5;60,0", false, "時刻 [s], 発熱量 [W] を１行ずつ入力"),
                    ]);
                    prims.extend([
                        Prim::Ellipse { x: -10.0, y: -10.0, w: 20.0, h: 20.0, pen: pen.clone(), brush: None },
                        line(0.0, 5.0, 0.0, -5.0),
                        line(0.0, -5.0, -4.0, -1.0),
                        line(0.0, -5.0, 4.0, -1.0),
                    ]);
                }
                "ThBoundary" => {
                    props.extend([
                        prop("温度 [°C]", "25", false, "定常解析の温度。一定モードでは過渡解析にも使用"),
                        prop("温度入力", "一定", false, "[一定, 温度表]"),
                        prop("温度表", "0,25;30,25;60,15", false, "時刻 [s], 温度 [°C] を１行ずつ入力"),
                    ]);
                    prims.extend([line(-14.0, -10.0, 14.0, -10.0), line(-10.0, -5.0, 10.0, -5.0), line(-6.0, 0.0, 6.0, 0.0)]);
                }
                _ => unreachable!(),
            }
            SymbolDef {
                shape: SymbolShape { prims, ports: vec![[0.0, -30.0]], bbox: [-14.0, -10.0, 14.0, 10.0], tx: 18.0, ty: -12.0, rotated: 0 },
                pin_names: vec!["1".into()],
                    }
        };
        let prefix = ["Rth", "Rc", "H", "Rad", "Cth", "P", "T", "Rax", "Rrad"][i];
        comps.push(CompInfo {
            typ: typ.into(), caption: label.into(), description: descriptions[i].into(), category: "thermal".into(),
            prefix: prefix.into(), props, pin_names: symbol.pin_names.clone(),
        });
        symbols.insert(typ.into(), symbol);
    }
}

/// Existing thermal schematics may omit properties added after they were saved.
pub fn fill_missing_properties(component: &mut Component, info: &CompInfo) {
    component.props.extend(info.props.iter().skip(component.props.len()).map(|p| crate::schematic::Prop {
        value: p.default.clone(),
        display: if p.display { "1" } else { "0" }.into(),
    }));
}

impl Library {
    pub fn info(&self, typ: &str) -> Option<&CompInfo> {
        self.by_type.get(typ).map(|&i| &self.comps[i])
    }
}

// ---------------------------------------------------------------------------
// Font metrics (approximation of the Qucs default UI font)
// ---------------------------------------------------------------------------

/// Qucs-S draws schematic text with its default 12 pt font (16 px at 96 dpi).
pub const FONT_PX: f64 = 16.0;
pub const LINE_H: f64 = 21.0;

pub fn text_width(s: &str, px: f64) -> f64 {
    let mut w = 0.0;
    for ch in s.chars() {
        w += match ch {
            'i' | 'l' | 'j' | '!' | '.' | ',' | ':' | ';' | '\'' | '|' | 'I' => 0.30,
            'f' | 't' | 'r' | '(' | ')' | '[' | ']' | ' ' | '-' => 0.38,
            'm' | 'w' | 'M' | 'W' => 0.85,
            'A'..='Z' => 0.66,
            c if (c as u32) > 0x2E80 => 1.0,
            _ => 0.56,
        };
    }
    w * px
}

/// Width/height of a (multi-line) text block.
pub fn text_size(s: &str, px: f64) -> (f64, f64) {
    let lines: Vec<&str> = s.split('\n').collect();
    let w = lines.iter().map(|l| text_width(l, px)).fold(0.0, f64::max);
    (w, lines.len() as f64 * px * 1.31)
}

// ---------------------------------------------------------------------------
// Placed symbols
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PlacedPin {
    pub index: usize,
    pub name: String,
    pub x: i32,
    pub y: i32,
    /// unit vector pointing away from the symbol body (where a wire should leave)
    pub dir: (i32, i32),
}

#[derive(Clone, Debug)]
pub struct Placed {
    pub prims: Vec<Prim>, // absolute coordinates
    pub pins: Vec<PlacedPin>,
    /// symbol body (absolute), without the text labels
    pub bbox: [f64; 4],
    /// label lines (name + visible properties) and their absolute top-left position
    pub label_lines: Vec<String>,
    pub label_pos: (f64, f64),
    pub label_bbox: Option<[f64; 4]>,
}

fn qucs_rotate_shape(prims: &mut [Prim], ports: &mut [[f64; 2]], bbox: &mut [f64; 4]) {
    for p in prims.iter_mut() {
        match p {
            Prim::Line { x1, y1, x2, y2, .. } => {
                let (a, b) = (*y1, -*x1);
                *x1 = a;
                *y1 = b;
                let (a, b) = (*y2, -*x2);
                *x2 = a;
                *y2 = b;
            }
            Prim::Ellipse { x, y, w, h, .. } => {
                let (nx, ny) = (*y, -*x - *w);
                *x = nx;
                *y = ny;
                std::mem::swap(w, h);
            }
        }
    }
    for q in ports.iter_mut() {
        *q = [q[1], -q[0]];
    }
    let [x1, y1, x2, y2] = *bbox;
    *bbox = [y1, -x2, y2, -x1];
}

fn qucs_mirror_shape(prims: &mut [Prim], ports: &mut [[f64; 2]], bbox: &mut [f64; 4]) {
    for p in prims.iter_mut() {
        match p {
            Prim::Line { y1, y2, .. } => {
                *y1 = -*y1;
                *y2 = -*y2;
            }
            Prim::Ellipse { y, h, .. } => *y = -*y - *h,
        }
    }
    for q in ports.iter_mut() {
        q[1] = -q[1];
    }
    let [x1, y1, x2, y2] = *bbox;
    *bbox = [x1, -y2, x2, -y1];
}

fn translate(p: &mut Prim, dx: f64, dy: f64) {
    match p {
        Prim::Line { x1, y1, x2, y2, .. } => {
            *x1 += dx;
            *x2 += dx;
            *y1 += dy;
            *y2 += dy;
        }
        Prim::Ellipse { x, y, .. } => {
            *x += dx;
            *y += dy;
        }
    }
}

/// Property name for display (`R=10k`).
pub fn prop_name(c: &Component, i: usize) -> String {
    let lib = lib();
    if let Some(info) = lib.info(&c.typ) {
        if let Some(p) = info.props.get(i) {
            return p.name.clone();
        }
    }
    format!("p{}", i + 1)
}

/// Text lines shown next to a component (name + displayed properties).
pub fn label_lines(c: &Component) -> Vec<String> {
    let mut lines = Vec::new();
    if c.show_name() && c.name != "*" && !c.name.is_empty() {
        lines.push(c.name.clone());
    }
    let branch_fields = crate::thermal::property_fields(c);
    for (i, p) in c.props.iter().enumerate() {
        if !p.visible() || branch_fields.is_some_and(|fields| !fields.contains(&i)) {
            continue;
        }
        if i == 0 && crate::thermal::is_element(&c.typ) {
            if !p.value.is_empty() {
                lines.push(p.value.clone());
            }
            continue;
        }
        lines.push(format!("{}={}", prop_name(c, i), p.value));
    }
    lines
}

/// Geometry of a component placed in a schematic (absolute coordinates).
pub fn place(c: &Component) -> Placed {
    let def = lib().symbols.get(&c.typ);
    let mut shape = def.map(|symbol| symbol.shape.clone()).unwrap_or_default();

    // Mirror first, then rotate from the symbol's default orientation.
    if c.mirror != 0 {
        qucs_mirror_shape(&mut shape.prims, &mut shape.ports, &mut shape.bbox);
    }
    for _ in 0..(c.rotate - shape.rotated).rem_euclid(4) {
        qucs_rotate_shape(&mut shape.prims, &mut shape.ports, &mut shape.bbox);
    }
    let SymbolShape { mut prims, ports, bbox, .. } = shape;
    let (cx, cy) = (c.x as f64, c.y as f64);
    for p in prims.iter_mut() {
        translate(p, cx, cy);
    }
    let names = def.map(|symbol| symbol.pin_names.as_slice()).unwrap_or_default();
    let body = [bbox[0] + cx, bbox[1] + cy, bbox[2] + cx, bbox[3] + cy];
    let pins: Vec<PlacedPin> = ports
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let (x, y) = ((q[0] + cx).round() as i32, (q[1] + cy).round() as i32);
            PlacedPin {
                index: i,
                name: names.get(i).cloned().unwrap_or_else(|| (i + 1).to_string()),
                x,
                y,
                dir: pin_direction(&prims, x, y, &body),
            }
        })
        .collect();
    let lines = label_lines(c);
    let label_pos = (cx + c.tx as f64, cy + c.ty as f64);
    let label_bbox = if lines.is_empty() {
        None
    } else {
        let w = lines.iter().map(|l| text_width(l, FONT_PX)).fold(0.0, f64::max);
        let h = lines.len() as f64 * LINE_H;
        Some([label_pos.0, label_pos.1, label_pos.0 + w, label_pos.1 + h])
    };
    Placed { prims, pins, bbox: body, label_lines: lines, label_pos, label_bbox }
}

/// Direction in which a wire should leave the pin: opposite to the symbol lead that ends there.
fn pin_direction(prims: &[Prim], x: i32, y: i32, body: &[f64; 4]) -> (i32, i32) {
    let (fx, fy) = (x as f64, y as f64);
    for p in prims {
        if let Prim::Line { x1, y1, x2, y2, .. } = p {
            let (ox, oy) = if (x1 - fx).abs() < 0.5 && (y1 - fy).abs() < 0.5 {
                (*x2, *y2)
            } else if (x2 - fx).abs() < 0.5 && (y2 - fy).abs() < 0.5 {
                (*x1, *y1)
            } else {
                continue;
            };
            let (dx, dy) = (fx - ox, fy - oy);
            if dx.abs() > dy.abs() && dx.abs() > 0.5 {
                return (dx.signum() as i32, 0);
            } else if dy.abs() > 0.5 {
                return (0, dy.signum() as i32);
            }
        }
    }
    // fall back to the side of the body the pin is closest to
    let cx = (body[0] + body[2]) / 2.0;
    let cy = (body[1] + body[3]) / 2.0;
    let d = [
        ((fx - body[0]).abs(), (-1, 0)),
        ((fx - body[2]).abs(), (1, 0)),
        ((fy - body[1]).abs(), (0, -1)),
        ((fy - body[3]).abs(), (0, 1)),
    ];
    let mut best = d[0];
    for e in d {
        if e.0 < best.0 {
            best = e;
        }
    }
    if (fx - cx).abs() < 1.0 && (fy - cy).abs() < 1.0 {
        return (0, 1);
    }
    best.1
}

/// Build a fresh component of `typ` with default properties.
pub fn new_component(typ: &str, name: &str, x: i32, y: i32) -> Result<Component, String> {
    let lib = lib();
    let info = lib.info(typ).ok_or_else(|| format!("unknown component type '{typ}' (see list_component_types)"))?;
    let shape = &lib.symbols[typ].shape;
    Ok(Component {
        typ: typ.into(),
        name: name.into(),
        active: 1,
        x,
        y,
        tx: shape.tx.round() as i32,
        ty: shape.ty.round() as i32,
        mirror: 0,
        rotate: shape.rotated & 3,
        props: info
            .props
            .iter()
            .map(|p| crate::schematic::Prop { value: p.default.clone(), display: if p.display { "1" } else { "0" }.into() })
            .collect(),
    })
}

/// Prefix for automatic component names.
pub fn name_prefix(typ: &str) -> String {
    lib().info(typ).map_or_else(|| typ.to_string(), |info| info.prefix.clone())
}
