//! Editable, axisymmetric thermal networks using the existing schematic elements.

use crate::library::{new_component, place};
use crate::schematic::{Component, Schematic, Wire};
use crate::thermal::{
    self, CAPACITY_CYLINDER, CAPACITY_DENSITY, CAPACITY_INNER, CAPACITY_LENGTH, CAPACITY_MODE,
    CAPACITY_OUTER, CAPACITY_SPECIFIC_HEAT, CONDUCTIVITY, CYLINDER_INNER_RADIUS, CYLINDER_LENGTH,
    CYLINDER_OUTER_RADIUS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_CELLS: usize = 256;

#[derive(Clone, Serialize, Deserialize)]
pub struct CylinderGrid {
    pub length_mm: f64,
    pub inner_mm: f64,
    pub axial: usize,
    pub layers: Vec<Layer>,
    pub surfaces: [Surface; 4],
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Layer {
    pub outer_mm: f64,
    pub radial: usize,
    pub material: Material,
    #[serde(default)]
    pub contact_to_inner: f64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub conductivity: f64,
    pub density: f64,
    pub specific_heat: f64,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "鋼（初期値）".into(),
            conductivity: 15.0,
            density: 7850.0,
            specific_heat: 480.0,
        }
    }
}

pub type MaterialLibrary = BTreeMap<String, Material>;

pub fn load_materials() -> Result<MaterialLibrary, String> {
    let Some(path) = crate::settings::config_dir().map(|p| p.join("thermal-materials.json")) else {
        return Ok(BTreeMap::new());
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("材料一覧を読み込めません（{e}）"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("材料一覧を読み込めません（{e}）")),
    }
}

pub fn register_material(material: &Material) -> Result<MaterialLibrary, String> {
    material.validate()?;
    let name = material.name.trim();
    if name.is_empty() {
        return Err("材料名を入力してください".into());
    }
    let dir = crate::settings::config_dir().ok_or("設定フォルダーが見つかりません")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("thermal-materials.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock()
        .map_err(|e| format!("材料一覧をロックできません（{e}）"))?;
    let mut library = load_materials()?;
    let mut material = material.clone();
    material.name = name.into();
    library.insert(material.name.clone(), material);
    let bytes = serde_json::to_vec_pretty(&library).map_err(|e| e.to_string())?;
    let tmp = dir.join("thermal-materials.json.tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|_| std::fs::rename(&tmp, dir.join("thermal-materials.json")))
        .map_err(|e| format!("材料を保存できません（{e}）"))?;
    Ok(library)
}

impl Material {
    fn validate(&self) -> Result<(), String> {
        for (value, label) in [
            (self.conductivity, "熱伝導率"),
            (self.density, "密度"),
            (self.specific_heat, "比熱"),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{label}は正の有限値にしてください"));
            }
        }
        Ok(())
    }
}

pub const SURFACE_NAMES: [&str; 4] = ["端面A", "端面B", "内面", "外面"];

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum SurfaceKind {
    #[default]
    Insulated,
    Temperature,
    Power,
    Convection,
    Radiation,
    ConvectionRadiation,
}

impl SurfaceKind {
    pub const OPTIONS: [(Self, &'static str); 6] = [
        (Self::Insulated, "断熱"),
        (Self::Temperature, "固定温度"),
        (Self::Power, "発熱"),
        (Self::Convection, "対流"),
        (Self::Radiation, "輻射"),
        (Self::ConvectionRadiation, "対流＋輻射"),
    ];

    pub fn convection(self) -> bool {
        matches!(self, Self::Convection | Self::ConvectionRadiation)
    }

    pub fn radiation(self) -> bool {
        matches!(self, Self::Radiation | Self::ConvectionRadiation)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Surface {
    pub kind: SurfaceKind,
    pub temperature: f64,
    pub power: f64,
    pub h: f64,
    pub emissivity: f64,
    pub surroundings: f64,
    pub power_profile: Profile,
    pub temperature_profile: Profile,
    pub surroundings_profile: Profile,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub enabled: bool,
    pub rows: String,
}

impl Profile {
    fn table(&self) -> Option<String> {
        self.enabled
            .then(|| self.rows.lines().collect::<Vec<_>>().join(";"))
    }
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            kind: SurfaceKind::Insulated,
            temperature: 25.0,
            power: 10.0,
            h: 10.0,
            emissivity: 0.8,
            surroundings: 25.0,
            power_profile: Profile::default(),
            temperature_profile: Profile::default(),
            surroundings_profile: Profile::default(),
        }
    }
}

impl Surface {
    fn validate(&self) -> Result<(), &'static str> {
        let valid_temp = |t: f64| t.is_finite() && t > -273.15;
        if (self.kind == SurfaceKind::Temperature || self.kind.convection())
            && !valid_temp(self.temperature)
        {
            return Err("温度は −273.15 °C より高くしてください");
        }
        if self.kind == SurfaceKind::Power && !self.power.is_finite() {
            return Err("発熱量は有限値にしてください");
        }
        if self.kind.convection() && !(self.h > 0.0 && self.h.is_finite()) {
            return Err("熱伝達率は正の有限値にしてください");
        }
        if self.kind.radiation()
            && !(self.emissivity > 0.0 && self.emissivity <= 1.0 && valid_temp(self.surroundings))
        {
            return Err(
                "有効放射率は 0 より大きく 1 以下、周囲の温度は −273.15 °C より高くしてください",
            );
        }
        Ok(())
    }
}

impl Default for CylinderGrid {
    fn default() -> Self {
        Self {
            length_mm: 100.0,
            inner_mm: 5.0,
            axial: 3,
            layers: vec![Layer {
                outer_mm: 10.0,
                radial: 2,
                material: Material::default(),
                contact_to_inner: 0.0,
            }],
            surfaces: std::array::from_fn(|_| Surface::default()),
        }
    }
}

type Point = (i32, i32);
const STEP_X: i32 = 280;
const STEP_Y: i32 = 260;
const END_PAD: i32 = 180;
const SURFACE_PAD: i32 = 180;
const CAP_JUNCTION: i32 = 50;
const GRID_SECTION: &str = "ThermalCylinder";
const CELL_KEY: usize = 10;

pub struct Ring<'a> {
    pub inner: f64,
    pub outer: f64,
    layer: usize,
    material: &'a Material,
}

impl Ring<'_> {
    fn center(&self) -> f64 {
        self.inner + (self.outer - self.inner) * 0.5
    }

    fn area_m2(&self) -> f64 {
        std::f64::consts::PI * (self.outer - self.inner) * (self.outer + self.inner) * 1e-6
    }
}

struct Patch {
    face: usize,
    name: String,
    area: f64,
}

#[derive(Default)]
struct SurfaceTables {
    power: Option<Vec<(f64, f64)>>,
    temperature: Option<String>,
    surroundings: Option<String>,
}

impl SurfaceTables {
    fn read(surface: &Surface) -> Result<Self, String> {
        let mut tables = Self::default();
        let temperatures = |profile: &Profile| {
            profile
                .table()
                .map(|table| {
                    thermal::temperature_schedule(&table)?;
                    Ok::<_, String>(table)
                })
                .transpose()
        };
        if surface.kind == SurfaceKind::Power {
            tables.power = surface
                .power_profile
                .table()
                .map(|table| thermal::schedule(&table, "発熱量 [W]"))
                .transpose()?;
        }
        if surface.kind == SurfaceKind::Temperature || surface.kind.convection() {
            tables.temperature = temperatures(&surface.temperature_profile)?;
        }
        if surface.kind.radiation() {
            tables.surroundings = temperatures(&surface.surroundings_profile)?;
        }
        Ok(tables)
    }
}

impl CylinderGrid {
    pub fn radial_count(&self) -> usize {
        self.layers
            .iter()
            .fold(0_usize, |n, layer| n.saturating_add(layer.radial))
    }

    pub fn rings(&self) -> impl Iterator<Item = Ring<'_>> {
        self.layers
            .iter()
            .enumerate()
            .flat_map(move |(layer, spec)| {
                let inner = if layer == 0 {
                    self.inner_mm
                } else {
                    self.layers[layer - 1].outer_mm
                };
                let dr = (spec.outer_mm - inner) / spec.radial as f64;
                (0..spec.radial).map(move |i| Ring {
                    inner: inner + i as f64 * dr,
                    outer: if i + 1 == spec.radial {
                        spec.outer_mm
                    } else {
                        inner + (i + 1) as f64 * dr
                    },
                    layer,
                    material: &spec.material,
                })
            })
    }

    pub fn read(sch: &Schematic) -> Option<Self> {
        let (_, lines) = sch
            .extra_sections
            .iter()
            .find(|(name, _)| name == GRID_SECTION)?;
        let grid: Self = serde_json::from_str(&lines.join("\n")).ok()?;
        grid.validate().ok()?;
        Some(grid)
    }

    fn validate(&self) -> Result<(), String> {
        if self.axial == 0
            || self.layers.is_empty()
            || self.layers.iter().any(|l| l.radial == 0)
            || self.axial.saturating_mul(self.radial_count()) > MAX_CELLS
        {
            return Err(format!(
                "分割数は各方向で 1 以上、合計 {MAX_CELLS} セル以下にしてください"
            ));
        }
        if !self.length_mm.is_finite() || self.length_mm <= 0.0 {
            return Err("軸長は正の有限値にしてください".into());
        }
        if !self.inner_mm.is_finite() || self.inner_mm < 0.0 {
            return Err("内半径は 0 以上にしてください".into());
        }
        let mut inner = self.inner_mm;
        for (i, layer) in self.layers.iter().enumerate() {
            if !layer.outer_mm.is_finite() || layer.outer_mm <= inner {
                return Err(format!(
                    "層 {} の外半径は内半径より大きくしてください",
                    i + 1
                ));
            }
            layer
                .material
                .validate()
                .map_err(|error| format!("層 {}: {error}", i + 1))?;
            if !layer.contact_to_inner.is_finite() || layer.contact_to_inner < 0.0 {
                return Err(format!(
                    "層 {} の接触熱抵抗は 0 以上の有限値にしてください",
                    i + 1
                ));
            }
            inner = layer.outer_mm;
        }
        for (face, surface) in self.surfaces.iter().enumerate() {
            if face != 2 || self.inner_mm > 0.0 {
                surface
                    .validate()
                    .map_err(|error| format!("{}: {error}", SURFACE_NAMES[face]))?;
            }
        }
        Ok(())
    }

    pub fn generate(&self) -> Result<Schematic, String> {
        self.validate()?;
        let dz = self.length_mm / self.axial as f64;
        let rings: Vec<_> = self.rings().collect();
        let last = rings.len() - 1;
        let cell =
            |z: usize, r: usize| (END_PAD + z as i32 * STEP_X, SURFACE_PAD + r as i32 * STEP_Y);
        let mut sch = thermal::empty();
        let mut patches = Vec::new();
        for (r, ring) in rings.iter().enumerate() {
            let limits = (ring.inner, ring.outer);
            let k = ring.material.conductivity;
            for z in 0..self.axial {
                let (x, y) = cell(z, r);
                let junction = (x, y + CAP_JUNCTION);
                self.capacity(&mut sch, z, r, junction, dz, ring)?;
                wire(&mut sch, (x, y), junction);
                if z + 1 < self.axial {
                    self.branch(&mut sch, (x, y), cell(z + 1, r), dz, limits, k, None, false)?;
                }
                if let Some(next) = rings.get(r + 1) {
                    let target = cell(z, r + 1);
                    if ring.layer == next.layer {
                        self.branch(
                            &mut sch,
                            junction,
                            target,
                            dz,
                            (ring.center(), next.center()),
                            k,
                            None,
                            true,
                        )?;
                    } else {
                        self.interface(&mut sch, junction, target, dz, z, ring, next)?;
                    }
                }
            }
            let start = cell(0, r);
            let end = cell(self.axial - 1, r);
            for (face, a, b) in [
                (0, (start.0 - END_PAD, start.1), start),
                (1, end, (end.0 + END_PAD, end.1)),
            ] {
                let name = format!("{}_層{}", SURFACE_NAMES[face], r + 1);
                self.branch(
                    &mut sch,
                    a,
                    b,
                    dz * 0.5,
                    limits,
                    k,
                    Some((face == 0, name.clone())),
                    false,
                )?;
                patches.push(Patch {
                    face,
                    name,
                    area: ring.area_m2(),
                });
            }
        }
        for z in 0..self.axial {
            let first = &rings[0];
            if self.inner_mm > 0.0 {
                let (x, y) = cell(z, 0);
                let name = format!("内面_区間{}", z + 1);
                self.branch(
                    &mut sch,
                    (x, y - SURFACE_PAD),
                    (x, y),
                    dz,
                    (self.inner_mm, first.center()),
                    first.material.conductivity,
                    Some((true, name.clone())),
                    true,
                )?;
                patches.push(Patch {
                    face: 2,
                    name,
                    area: std::f64::consts::TAU * self.inner_mm * dz * 1e-6,
                });
            }
            let ring = &rings[last];
            let (x, y) = cell(z, last);
            let name = format!("外面_区間{}", z + 1);
            self.branch(
                &mut sch,
                (x, y + CAP_JUNCTION),
                (x, y + SURFACE_PAD),
                dz,
                (ring.center(), ring.outer),
                ring.material.conductivity,
                Some((false, name.clone())),
                true,
            )?;
            patches.push(Patch {
                face: 3,
                name,
                area: std::f64::consts::TAU * ring.outer * dz * 1e-6,
            });
        }
        append_conditions(
            &mut sch,
            &patches,
            &self.surfaces,
            cell(0, last).1 + SURFACE_PAD + 200,
        )?;
        let data = serde_json::to_string(self).map_err(|error| error.to_string())?;
        sch.extra_sections.push((GRID_SECTION.into(), vec![data]));
        Ok(sch)
    }

    fn capacity(
        &self,
        sch: &mut Schematic,
        z: usize,
        r: usize,
        junction: Point,
        length: f64,
        ring: &Ring<'_>,
    ) -> Result<(), String> {
        let mut c = new_component(
            "ThCapacity",
            &format!("C{}_{}", z + 1, r + 1),
            junction.0 + 70,
            junction.1 + 30,
        )?;
        c.props[0].value.clear();
        c.props[CAPACITY_MODE].value = CAPACITY_CYLINDER.into();
        c.props.push(crate::schematic::Prop {
            value: format!("{z}:{r}"),
            display: "0".into(),
        });
        set_dimensions(
            &mut c,
            [
                (CAPACITY_LENGTH, length),
                (CAPACITY_INNER, ring.inner),
                (CAPACITY_OUTER, ring.outer),
                (CAPACITY_DENSITY, ring.material.density),
                (CAPACITY_SPECIFIC_HEAT, ring.material.specific_heat),
            ],
        );
        thermal::capacity_from_dimensions(&c).expect("cylindrical capacity")?;
        let pin = &place(&c, None).pins[0];
        wire(sch, junction, (pin.x, pin.y));
        sch.components.push(c);
        Ok(())
    }

    fn branch(
        &self,
        sch: &mut Schematic,
        start: Point,
        end: Point,
        length: f64,
        radii: (f64, f64),
        conductivity: f64,
        surface: Option<(bool, String)>,
        radial: bool,
    ) -> Result<(), String> {
        let (typ, prefix, label) = if radial {
            ("ThCylinderRadial", "Rr", "半径")
        } else {
            ("ThCylinderAxial", "Rz", "軸")
        };
        let mut c = new_component(
            typ,
            &format!("{prefix}{}", sch.components.len() + 1),
            crate::doc::snap((start.0 + end.0) / 2),
            crate::doc::snap((start.1 + end.1) / 2),
        )?;
        c.props[0].value = label.into();
        if radial {
            c.rotate = 3;
            (c.tx, c.ty) = (14, -17);
        } else if surface.as_ref().is_some_and(|(at_start, _)| !at_start) {
            c.tx = 4;
        }
        set_dimensions(
            &mut c,
            [
                (CYLINDER_LENGTH, length),
                (CYLINDER_INNER_RADIUS, radii.0),
                (CYLINDER_OUTER_RADIUS, radii.1),
                (CONDUCTIVITY, conductivity),
            ],
        );
        thermal::resistance_from_dimensions(&c).expect("cylindrical resistance")?;
        connect_branch(sch, c, start, end, radial, surface)
    }

    fn interface(
        &self,
        sch: &mut Schematic,
        start: Point,
        end: Point,
        dz: f64,
        z: usize,
        ring: &Ring<'_>,
        next: &Ring<'_>,
    ) -> Result<(), String> {
        let contact = self.layers[next.layer].contact_to_inner;
        let joint = |portion: i32, segments: i32| {
            (
                start.0,
                crate::doc::snap(start.1 + (end.1 - start.1) * portion / segments),
            )
        };
        let (inside, outside) = if contact == 0.0 {
            let point = joint(1, 2);
            (point, point)
        } else {
            (joint(1, 3), joint(2, 3))
        };
        self.branch(
            sch,
            start,
            inside,
            dz,
            (ring.center(), ring.outer),
            ring.material.conductivity,
            None,
            true,
        )?;
        if contact > 0.0 {
            let resistance = contact / (std::f64::consts::TAU * ring.outer * dz * 1e-6);
            if !(resistance > 0.0 && resistance.is_finite() && (1.0 / resistance).is_finite()) {
                return Err("界面の面積から有効な接触熱抵抗を計算できません".into());
            }
            let (x, y) = joint(1, 2);
            let mut c = new_component(
                "ThContact",
                &format!("Rc{}", sch.components.len() + 1),
                x,
                y,
            )?;
            c.rotate = 3;
            (c.tx, c.ty) = (14, -17);
            c.props[2].value = resistance.to_string();
            connect_branch(sch, c, inside, outside, true, None)?;
            for (point, name) in [(inside, "内側"), (outside, "外側")] {
                let mut stub = Wire::new(point.0, point.1, point.0, point.1);
                stub.set_label(
                    &format!("界面{}_区間{}_{name}", next.layer, z + 1),
                    point.0 - 164,
                    point.1 - 12,
                    point,
                );
                sch.wires.push(stub);
            }
        }
        self.branch(
            sch,
            outside,
            end,
            dz,
            (next.inner, next.center()),
            next.material.conductivity,
            None,
            true,
        )
    }
}

fn connect_branch(
    sch: &mut Schematic,
    c: Component,
    start: Point,
    end: Point,
    radial: bool,
    surface: Option<(bool, String)>,
) -> Result<(), String> {
    let placed = place(&c, None);
    let [first, second] = placed.pins.as_slice() else {
        return Err("円筒素子の端子を配置できません".into());
    };
    for (on_start, from, to) in [
        (true, start, (first.x, first.y)),
        (false, (second.x, second.y), end),
    ] {
        let mut wire = Wire::new(from.0, from.1, to.0, to.1);
        if let Some((_, name)) = surface
            .as_ref()
            .filter(|(at_start, _)| *at_start == on_start)
        {
            let (x, y) = if on_start { start } else { end };
            let lx = if radial || on_start { x + 8 } else { x - 92 };
            let ly = if radial && !on_start {
                y + 6
            } else if radial {
                y - 30
            } else {
                y - 48
            };
            wire.set_label(name, lx, ly, (x, y));
        }
        if !wire.is_point() || !wire.label.is_empty() {
            sch.wires.push(wire);
        }
    }
    sch.components.push(c);
    Ok(())
}

fn set_dimensions<const N: usize>(component: &mut Component, values: [(usize, f64); N]) {
    for (index, value) in values {
        component.props[index].value = value.to_string();
    }
}

fn wire(sch: &mut Schematic, start: Point, end: Point) {
    sch.wires.push(Wire::new(start.0, start.1, end.0, end.1));
}

fn append_conditions(
    sch: &mut Schematic,
    patches: &[Patch],
    surfaces: &[Surface; 4],
    top: i32,
) -> Result<(), String> {
    let mut face_areas = [0.0; 4];
    for patch in patches {
        face_areas[patch.face] += patch.area;
    }
    let mut tables: [SurfaceTables; 4] = std::array::from_fn(|_| SurfaceTables::default());
    for (face, surface) in surfaces.iter().enumerate() {
        if face_areas[face] > 0.0 {
            tables[face] = SurfaceTables::read(surface)
                .map_err(|error| format!("{}: {error}", SURFACE_NAMES[face]))?;
        }
    }
    let mut index = 0;
    for patch in patches {
        let surface = &surfaces[patch.face];
        let tables = &tables[patch.face];
        let mut position = || {
            let p = (110 + (index % 3) * 420, top + (index / 3) * 140);
            index += 1;
            p
        };
        match surface.kind {
            SurfaceKind::Temperature => {
                surface_terminal(
                    sch,
                    "ThBoundary",
                    "T",
                    "境界",
                    position(),
                    &patch.name,
                    surface.temperature,
                    tables.temperature.as_deref(),
                )?;
            }
            SurfaceKind::Power => {
                let share = patch.area / face_areas[patch.face];
                let power = surface.power * share;
                let schedule = tables.power.as_ref().map(|points| {
                    points
                        .iter()
                        .map(|(time, power)| format!("{time},{}", power * share))
                        .collect::<Vec<_>>()
                        .join(";")
                });
                surface_terminal(
                    sch,
                    "ThPower",
                    "P",
                    "発熱",
                    position(),
                    &patch.name,
                    power,
                    schedule.as_deref(),
                )?;
            }
            _ => {}
        }
        if surface.kind.convection() {
            surface_exchange(
                sch,
                patch,
                position(),
                "ThConvection",
                "対流",
                surface.h,
                surface.temperature,
                tables.temperature.as_deref(),
            )?;
        }
        if surface.kind.radiation() {
            surface_exchange(
                sch,
                patch,
                position(),
                "ThRadiation",
                "輻射",
                surface.emissivity,
                surface.surroundings,
                tables.surroundings.as_deref(),
            )?;
        }
    }
    Ok(())
}

fn surface_terminal(
    sch: &mut Schematic,
    typ: &str,
    prefix: &str,
    label: &str,
    at: Point,
    net: &str,
    value: f64,
    schedule: Option<&str>,
) -> Result<(), String> {
    let mut component = new_component(
        typ,
        &format!("{prefix}{}", sch.components.len() + 1),
        at.0,
        at.1,
    )?;
    component.props[0].value = label.into();
    component.props[1].value = value.to_string();
    apply_profile(&mut component, schedule);
    let pin = &place(&component, None).pins[0];
    let mut wire = Wire::new(pin.x - 80, pin.y, pin.x, pin.y);
    wire.set_label(net, pin.x - 75, pin.y - 35, (pin.x - 80, pin.y));
    sch.wires.push(wire);
    sch.components.push(component);
    Ok(())
}

fn surface_exchange(
    sch: &mut Schematic,
    patch: &Patch,
    at: Point,
    typ: &str,
    label: &str,
    coefficient: f64,
    temperature: f64,
    schedule: Option<&str>,
) -> Result<(), String> {
    let mut c = new_component(typ, &format!("H{}", sch.components.len() + 1), at.0, at.1)?;
    c.props[0].value = label.into();
    set_dimensions(
        &mut c,
        [
            (4, patch.area),
            (if typ == "ThConvection" { 3 } else { 5 }, coefficient),
        ],
    );
    let placed = place(&c, None);
    let first = &placed.pins[0];
    let second = &placed.pins[1];
    let mut left = Wire::new(first.x - 80, first.y, first.x, first.y);
    left.set_label(
        &patch.name,
        first.x - 75,
        first.y - 36,
        (first.x - 80, first.y),
    );
    sch.wires.push(left);
    sch.components.push(c);
    let mut t = new_component(
        "ThBoundary",
        &format!("T{}", sch.components.len() + 1),
        second.x + 80,
        second.y + 30,
    )?;
    t.props[0].value = if typ == "ThConvection" {
        "流体温度".into()
    } else {
        "周囲温度".into()
    };
    t.props[1].value = temperature.to_string();
    apply_profile(&mut t, schedule);
    let pin = &place(&t, None).pins[0];
    wire(sch, (second.x, second.y), (pin.x, pin.y));
    sch.components.push(t);
    Ok(())
}

fn apply_profile(component: &mut Component, schedule: Option<&str>) {
    if let Some(schedule) = schedule {
        let mode = if component.typ == "ThPower" {
            "発熱表"
        } else {
            thermal::BOUNDARY_TABLE
        };
        component.props[2].value = mode.into();
        component.props[3].value = schedule.into();
    }
}

#[derive(Clone)]
pub struct TemperatureMap {
    pub grid: CylinderGrid,
    pub signals: Vec<Option<usize>>,
}

impl TemperatureMap {
    pub fn new(
        sch: &Schematic,
        conn: &crate::netlist::Connectivity,
        result: &thermal::Results,
    ) -> Option<Self> {
        let grid = CylinderGrid::read(sch)?;
        let radial = grid.radial_count();
        let mut signals = vec![None; grid.axial * radial];
        let mut assigned = vec![false; signals.len()];
        let by_net: BTreeMap<_, _> = result
            .temperatures_c
            .iter()
            .enumerate()
            .map(|(i, s)| (s.index, i))
            .collect();
        for pin in &conn.pins {
            let component = &sch.components[pin.comp];
            if component.typ != "ThCapacity" || component.active & 3 == 0 {
                continue;
            }
            let Some((z, r)) = thermal::property(component, CELL_KEY).split_once(':') else {
                continue;
            };
            let (Ok(z), Ok(r)) = (z.parse::<usize>(), r.parse::<usize>()) else {
                continue;
            };
            if z < grid.axial && r < radial {
                let cell = r * grid.axial + z;
                signals[cell] = if assigned[cell] {
                    None
                } else {
                    by_net.get(&pin.net).copied()
                };
                assigned[cell] = true;
            }
        }
        Some(Self { grid, signals })
    }

    pub fn signal_at(&self, z_mm: f64, r_mm: f64) -> Option<usize> {
        if !(0.0..=self.grid.length_mm).contains(&z_mm) {
            return None;
        }
        let z = ((z_mm / self.grid.length_mm * self.grid.axial as f64) as usize)
            .min(self.grid.axial - 1);
        let r = self
            .grid
            .rings()
            .position(|ring| (ring.inner..=ring.outer).contains(&r_mm))?;
        self.signals[r * self.grid.axial + z]
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub enum SweepParameter {
    #[default]
    OuterThickness,
    OuterConvection,
    Mesh,
}

impl SweepParameter {
    pub const OPTIONS: [(Self, &'static str); 3] = [
        (Self::OuterThickness, "最外層の厚さ [mm]"),
        (Self::OuterConvection, "外面の熱伝達率 [W/(m²·K)]"),
        (Self::Mesh, "両方向の分割倍率"),
    ];
}

#[derive(Clone)]
pub struct Sweep {
    pub parameter: SweepParameter,
    pub from: f64,
    pub to: f64,
    pub count: usize,
    pub steady: bool,
    pub observe_z: f64,
    pub observe_r: f64,
}

impl Default for Sweep {
    fn default() -> Self {
        Self {
            parameter: SweepParameter::default(),
            from: 2.0,
            to: 10.0,
            count: 5,
            steady: true,
            observe_z: 50.0,
            observe_r: 6.0,
        }
    }
}

pub struct SweepRow {
    pub value: f64,
    pub cells: usize,
    pub result: Result<SweepMetrics, String>,
}

pub struct SweepMetrics {
    pub temperature: f64,
    pub release: f64,
    pub observed: Option<f64>,
}

impl Sweep {
    pub fn run(
        &self,
        base: &CylinderGrid,
        settings: &thermal::Settings,
    ) -> Result<Vec<SweepRow>, String> {
        base.validate()?;
        if !self.from.is_finite()
            || !self.to.is_finite()
            || self.from <= 0.0
            || self.to <= self.from
            || !(2..=50).contains(&self.count)
        {
            return Err(
                "正の開始値、開始値より大きい終了値、2〜50 の試行数を指定してください".into(),
            );
        }
        if self.parameter == SweepParameter::Mesh
            && (self.from.fract() != 0.0 || self.to.fract() != 0.0 || self.to > MAX_CELLS as f64)
        {
            return Err("分割倍率は正の整数で指定してください".into());
        }
        if self.parameter == SweepParameter::OuterConvection && !base.surfaces[3].kind.convection()
        {
            return Err("外面の表面条件に対流を設定してください".into());
        }
        let mut values: Vec<_> = (0..self.count)
            .map(|i| {
                let value = self.from + (self.to - self.from) * i as f64 / (self.count - 1) as f64;
                if self.parameter == SweepParameter::Mesh {
                    value.round()
                } else {
                    value
                }
            })
            .collect();
        values.dedup();
        Ok(values
            .into_iter()
            .map(|value| {
                let mut grid = base.clone();
                match self.parameter {
                    SweepParameter::OuterThickness => {
                        let last = grid.layers.len() - 1;
                        let inner = if last == 0 {
                            grid.inner_mm
                        } else {
                            grid.layers[last - 1].outer_mm
                        };
                        grid.layers[last].outer_mm = inner + value;
                    }
                    SweepParameter::OuterConvection => grid.surfaces[3].h = value,
                    SweepParameter::Mesh => {
                        grid.axial *= value as usize;
                        for layer in &mut grid.layers {
                            layer.radial *= value as usize;
                        }
                    }
                }
                let cells = grid.axial * grid.radial_count();
                let result = self.run_case(&grid, settings);
                SweepRow {
                    value,
                    cells,
                    result,
                }
            })
            .collect())
    }

    fn run_case(
        &self,
        grid: &CylinderGrid,
        settings: &thermal::Settings,
    ) -> Result<SweepMetrics, String> {
        let sch = grid.generate()?;
        let conn = crate::netlist::Connectivity::build(&sch, None);
        let network = thermal::Network::build(&sch, &conn)?;
        let results = if self.steady {
            network.steady()
        } else {
            network.transient(settings)
        }?;
        let observed = TemperatureMap::new(&sch, &conn, &results)
            .and_then(|map| map.signal_at(self.observe_z, self.observe_r))
            .map(|index| thermal::Results::peak(&results.temperatures_c[index]));
        Ok(SweepMetrics {
            temperature: results.peak_temperature_c,
            release: results.peak_release_w,
            observed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::Connectivity;
    use crate::thermal::Network;

    fn with_boundaries(mut sch: Schematic, hot: &str, cold: &str) -> Schematic {
        for wire in &sch.wires {
            let temperature = if wire.label.starts_with(hot) {
                "45"
            } else if wire.label.starts_with(cold) {
                "25"
            } else {
                continue;
            };
            let (x, y) = wire.label_anchor();
            let mut boundary = new_component(
                "ThBoundary",
                &format!("T{}", sch.components.len()),
                x,
                y + 30,
            )
            .unwrap();
            boundary.props[1].value = temperature.into();
            sch.components.push(boundary);
        }
        sch
    }

    #[test]
    fn grid_reproduces_axial_and_radial_heat_flow_and_totals_the_wall_capacity() {
        let grid = CylinderGrid::default();
        let sch = grid.generate().unwrap();
        assert_eq!(sch, Schematic::parse(&sch.serialize()).unwrap());
        let capacities: Vec<_> = sch
            .components
            .iter()
            .filter_map(thermal::capacity_from_dimensions)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(capacities.len(), grid.axial * grid.radial_count());
        let area = std::f64::consts::PI * (0.01_f64.powi(2) - 0.005_f64.powi(2));
        let total =
            grid.layers[0].material.density * grid.layers[0].material.specific_heat * area * 0.1;
        assert!((capacities.iter().sum::<f64>() - total).abs() < 1e-8);
        let conn = Connectivity::build(&sch, None);
        let issues = crate::check::check(&sch, &conn).issues;
        assert!(
            issues.is_empty(),
            "{:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        for (hot, cold, expected) in [
            (
                "端面A",
                "端面B",
                grid.layers[0].material.conductivity * area / 0.1 * 20.0,
            ),
            (
                "内面",
                "外面",
                std::f64::consts::TAU * grid.layers[0].material.conductivity * 0.1 * 20.0
                    / 2.0_f64.ln(),
            ),
        ] {
            let sch = with_boundaries(sch.clone(), hot, cold);
            let results = Network::build(&sch, &Connectivity::build(&sch, None))
                .unwrap()
                .steady()
                .unwrap();
            assert!(
                (results.peak_release_w - expected).abs() < 1e-6,
                "expected {expected}, got {}",
                results.peak_release_w
            );
        }
    }

    #[test]
    fn solid_cylinder_generates_without_an_inner_surface() {
        let sch = CylinderGrid {
            inner_mm: 0.0,
            axial: 1,
            layers: vec![Layer {
                radial: 1,
                ..CylinderGrid::default().layers.remove(0)
            }],
            ..CylinderGrid::default()
        }
        .generate()
        .unwrap();
        assert!(!sch.wires.iter().any(|w| w.label.starts_with("内面")));
        let connected = with_boundaries(sch, "端面A", "外面");
        let output = Network::build(&connected, &Connectivity::build(&connected, None))
            .unwrap()
            .steady()
            .unwrap();
        assert!(output.peak_release_w > 0.0);
    }

    #[test]
    fn layered_wall_has_the_radial_resistance_of_both_materials() {
        let mut grid = CylinderGrid {
            axial: 2,
            ..CylinderGrid::default()
        };
        grid.layers.push(Layer {
            outer_mm: 20.0,
            radial: 2,
            material: Material {
                name: "insulation".into(),
                conductivity: 0.04,
                density: 100.0,
                specific_heat: 800.0,
            },
            contact_to_inner: 0.0,
        });
        grid.surfaces[2].kind = SurfaceKind::Temperature;
        grid.surfaces[2].temperature = 45.0;
        grid.surfaces[3].kind = SurfaceKind::Temperature;
        let sch = Schematic::parse(&grid.generate().unwrap().serialize()).unwrap();
        let conn = Connectivity::build(&sch, None);
        let output = Network::build(&sch, &conn).unwrap().steady().unwrap();
        let expected =
            20.0 * std::f64::consts::TAU * 0.1 / (2.0_f64.ln() / 15.0 + 2.0_f64.ln() / 0.04);
        assert!((output.peak_release_w - expected).abs() < 1e-7);
        let map = TemperatureMap::new(&sch, &conn, &output).unwrap();
        let inner = map.signal_at(25.0, 6.0).unwrap();
        let outer = map.signal_at(25.0, 19.0).unwrap();
        assert!(output.temperatures_c[inner].values[0] > output.temperatures_c[outer].values[0]);
        assert!(map.signal_at(25.0, 21.0).is_none());
    }

    #[test]
    fn surface_power_and_areas_survive_mesh_refinement() {
        let mut grid = CylinderGrid::default();
        grid.surfaces[0].kind = SurfaceKind::Power;
        grid.surfaces[0].power = 12.0;
        grid.surfaces[3].kind = SurfaceKind::ConvectionRadiation;
        let sch = grid.generate().unwrap();
        let sum = |typ: &str, index: usize| {
            sch.components
                .iter()
                .filter(|c| c.typ == typ)
                .map(|c| c.props[index].value.parse::<f64>().unwrap())
                .sum::<f64>()
        };
        let area = std::f64::consts::TAU * 0.01 * 0.1;
        assert!((sum("ThPower", 1) - 12.0).abs() < 1e-10);
        assert!((sum("ThConvection", 4) - area).abs() < 1e-10);
        assert!((sum("ThRadiation", 4) - area).abs() < 1e-10);
        let conn = Connectivity::build(&sch, None);
        let issues = crate::check::check(&sch, &conn).issues;
        assert!(
            issues.is_empty(),
            "{:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );

        let sweep = Sweep {
            parameter: SweepParameter::Mesh,
            from: 1.0,
            to: 2.0,
            count: 2,
            ..Sweep::default()
        };
        let rows = sweep.run(&grid, &thermal::Settings::default()).unwrap();
        assert_eq!(rows[0].cells, 6);
        assert_eq!(rows[1].cells, 24);
        for row in rows {
            let metrics = row.result.unwrap();
            assert!((metrics.release - 12.0).abs() < 1e-6);
            assert!(metrics.observed.is_some());
        }
        let sweep = Sweep {
            parameter: SweepParameter::OuterConvection,
            from: 8.0,
            to: 25.0,
            count: 2,
            ..Sweep::default()
        };
        let rows = sweep.run(&grid, &thermal::Settings::default()).unwrap();
        assert!(
            rows[0].result.as_ref().unwrap().temperature
                > rows[1].result.as_ref().unwrap().temperature
        );
    }

    #[test]
    fn generated_contact_adds_resistance_and_separates_the_interface_temperatures() {
        let mut grid = CylinderGrid {
            axial: 2,
            ..CylinderGrid::default()
        };
        let mut outer = grid.layers[0].clone();
        outer.outer_mm = 20.0;
        outer.contact_to_inner = 0.002;
        grid.layers.push(outer);
        grid.surfaces[2].kind = SurfaceKind::Temperature;
        grid.surfaces[2].temperature = 45.0;
        grid.surfaces[3].kind = SurfaceKind::Temperature;
        let sch = Schematic::parse(&grid.generate().unwrap().serialize()).unwrap();
        let conn = Connectivity::build(&sch, None);
        let issues = crate::check::check(&sch, &conn).issues;
        assert!(
            issues.is_empty(),
            "{:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        let output = Network::build(&sch, &conn).unwrap().steady().unwrap();
        let perimeter_length = std::f64::consts::TAU * 0.1;
        let resistance =
            4.0_f64.ln() / (perimeter_length * 15.0) + 0.002 / (perimeter_length * 0.01);
        let expected = 20.0 / resistance;
        assert!((output.peak_release_w - expected).abs() < 1e-6);
        let temperature = |name| {
            output
                .temperatures_c
                .iter()
                .find(|s| s.name == name)
                .unwrap()
                .values[0]
        };
        let jump = temperature("界面1_区間1_内側") - temperature("界面1_区間1_外側");
        assert!((jump - expected * 0.002 / (perimeter_length * 0.01)).abs() < 1e-7);
        assert_eq!(
            CylinderGrid::read(&sch).unwrap().layers[1].contact_to_inner,
            0.002
        );
    }

    #[test]
    fn generated_profiles_preserve_total_power_and_independent_temperature_tables() {
        let mut grid = CylinderGrid::default();
        let hot = &mut grid.surfaces[0];
        hot.kind = SurfaceKind::Power;
        hot.power = 4.0;
        hot.power_profile = Profile {
            enabled: true,
            rows: "0,0\n1,12\n2,0".into(),
        };
        let cold = &mut grid.surfaces[3];
        cold.kind = SurfaceKind::ConvectionRadiation;
        cold.temperature_profile = Profile {
            enabled: true,
            rows: "0,25\n2,15".into(),
        };
        cold.surroundings_profile = Profile {
            enabled: true,
            rows: "0,30\n2,50".into(),
        };
        let sch = Schematic::parse(&grid.generate().unwrap().serialize()).unwrap();
        let grid = CylinderGrid::read(&sch).unwrap();
        assert_eq!(grid.surfaces[0].power_profile.rows, "0,0\n1,12\n2,0");
        let sch = grid.generate().unwrap();
        let conn = Connectivity::build(&sch, None);
        let network = Network::build(&sch, &conn).unwrap();
        assert!((network.steady().unwrap().balance_w[0].values[0] - 4.0).abs() < 1e-10);
        let output = network
            .transient(&thermal::Settings {
                end_s: 3.0,
                ..thermal::Settings::default()
            })
            .unwrap();
        let at = |time| output.time_s.iter().rposition(|&t| t == time).unwrap();
        assert!((output.balance_w[0].values[at(1.0)] - 12.0).abs() < 1e-10);
        assert!(output.balance_w[0].values[at(2.0)].abs() < 1e-10);
        for (label, expected) in [("流体温度", 20.0), ("周囲温度", 40.0)] {
            let pin = conn
                .pins
                .iter()
                .find(|pin| thermal::property(&sch.components[pin.comp], 0) == label)
                .unwrap();
            let series = output
                .temperatures_c
                .iter()
                .find(|s| s.index == pin.net)
                .unwrap();
            assert!((series.values[at(1.0)] - expected).abs() < 1e-10);
        }
    }
}
