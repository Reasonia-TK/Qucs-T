use super::{b, s, text, tool, workdir, Doc, PathBuf, Schematic, Server, Value};
use crate::thermal::{self, Network, Results, Series, Settings};
use crate::thermal_grid::{CylinderGrid, Layer, Surface, Sweep, SweepParameter, TemperatureMap};
use serde::Deserialize;
use serde_json::json;
use std::io::Write;

pub(super) struct ThermalRun {
    kind: &'static str,
    snapshot: Schematic,
    settings: Settings,
    results: Results,
    map: Option<TemperatureMap>,
}

const GROUPS: [&str; 4] = ["temperature", "branch_heat", "boundary_heat", "balance"];

fn settings_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {
        "initial_c": {"type": "number"}, "start_at_steady": {"type": "boolean"},
        "end_s": {"type": "number"}, "output_step_s": {"type": "number"}, "threshold_c": {"type": "number"}
    }})
}

pub(super) fn tools() -> Vec<Value> {
    let profile = json!({"type":"object", "required":["enabled","rows"], "properties":{
        "enabled":{"type":"boolean"}, "rows":{"type":"string", "description":"Newline-separated time,value pairs, starting at 0 seconds"}
    }});
    let surface = json!({"type":"object", "properties":{
        "kind":{"type":"string","enum":["Insulated","Temperature","Power","Convection","Radiation","ConvectionRadiation"]},
        "temperature":{"type":"number","description":"Surface or fluid temperature in °C"},
        "power":{"type":"number","description":"Total surface power in W"},
        "h":{"type":"number","description":"W/(m²·K)"},"emissivity":{"type":"number"},"surroundings":{"type":"number","description":"Radiative surroundings temperature in °C"},
        "power_profile":profile,"temperature_profile":profile,"surroundings_profile":profile
    }});
    let config = json!({"type":"object","required":["length_mm","inner_mm","axial","layers"],"properties":{
        "length_mm":{"type":"number"},"inner_mm":{"type":"number"},"axial":{"type":"integer"},
        "layers":{"type":"array","items":{"type":"object","required":["outer_mm","radial","material"],"properties":{
            "outer_mm":{"type":"number"},"radial":{"type":"integer"},"contact_to_inner":{"type":"number","description":"Optional area-specific contact resistance in m²·K/W"},
            "material":{"type":"object","required":["name","conductivity","density","specific_heat"],"properties":{
                "name":{"type":"string"},"conductivity":{"type":"number","description":"W/(m·K)"},"density":{"type":"number","description":"kg/m³"},"specific_heat":{"type":"number","description":"J/(kg·K)"}
            }}
        }}},
        "surfaces":{"type":"object","properties":{"end_a":surface,"end_b":surface,"inner":surface,"outer":surface},"description":"Omitted surfaces are insulated."}
    }});
    let kind = json!({"type":"string","enum":["steady","transient"]});
    let point = json!({"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2,"description":"[axial position, radius] in mm; generated cylinders only"});
    vec![
        tool("new_thermal_schematic", "Create and open an empty Qucs-T .qucst file. Use list_component_types with category thermal, add_components, update_component and connect to edit it.",
            json!({"path":{"type":"string"},"overwrite":{"type":"boolean"}}), &["path"]),
        tool("generate_thermal_cylinder", "Generate and open an editable Qucs-T cylinder including layers, contact resistance, surface conditions and time profiles. Uses the same generator as the GUI.",
            json!({"path":{"type":"string"},"overwrite":{"type":"boolean"},"config":config}), &["path","config"]),
        tool("get_thermal_settings", "Read saved analysis settings from the open thermal schematic.", json!({}), &[]),
        tool("set_thermal_settings", "Change analysis settings and save them in the open thermal schematic.", json!({"settings":settings_schema()}), &["settings"]),
        tool("run_thermal_analysis", "Run the built-in Qucs-T steady or transient solver on the open .qucst. Optional settings override the saved settings for this run. Use get_thermal_results for values or export_thermal_csv for a complete CSV.",
            json!({"kind":kind,"settings":settings_schema()}), &["kind"]),
        tool("get_thermal_results", "List available thermal signals and steady values, or read one signal by exact name, zero-based signal_index, or position_mm. Use at_s for interpolation, or offset and limit for raw samples.",
            json!({"group":{"type":"string","enum":GROUPS},"signal":{"type":"string"},"signal_index":{"type":"integer","minimum":0},
                "position_mm":point,"at_s":{"type":"array","items":{"type":"number"},"maxItems":256},
                "offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":1000}}), &[]),
        tool("export_thermal_csv", "Write all signals from the current thermal analysis to a new CSV file using UTF-8.",
            json!({"path":{"type":"string","description":"New .csv file; relative paths are resolved against QUCS_MCP_WORKDIR"}}), &["path"]),
        tool("run_thermal_sweep", "Regenerate and analyze the saved cylinder configuration at each sweep value. Optionally report temperature at the same physical observation point for every case.",
            json!({"parameter":{"type":"string","enum":["outer_thickness","outer_convection","mesh_multiplier"]},"from":{"type":"number"},"to":{"type":"number"},
                "count":{"type":"integer","minimum":2,"maximum":50},"kind":kind,"settings":settings_schema(),"position_mm":point}),
            &["parameter","from","to","count"]),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CylinderArgs {
    length_mm: f64,
    inner_mm: f64,
    axial: usize,
    layers: Vec<Layer>,
    #[serde(default)]
    surfaces: SurfaceArgs,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SurfaceArgs {
    end_a: Surface,
    end_b: Surface,
    inner: Surface,
    outer: Surface,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SettingsPatch {
    initial_c: Option<f64>,
    start_at_steady: Option<bool>,
    end_s: Option<f64>,
    output_step_s: Option<f64>,
    threshold_c: Option<f64>,
}

fn settings(sch: &Schematic, a: &Value) -> Result<Settings, String> {
    let current = Settings::read(sch)?;
    let patch: SettingsPatch = match a.get("settings") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| format!("invalid analysis settings: {e}"))?,
        None => SettingsPatch::default(),
    };
    let settings = Settings {
        initial_c: patch.initial_c.unwrap_or(current.initial_c),
        start_at_steady: patch.start_at_steady.unwrap_or(current.start_at_steady),
        end_s: patch.end_s.unwrap_or(current.end_s),
        output_step_s: patch.output_step_s.unwrap_or(current.output_step_s),
        threshold_c: patch.threshold_c.unwrap_or(current.threshold_c),
    };
    if settings.initial_c <= -273.15 || settings.threshold_c <= -273.15 {
        return Err("initial_c and threshold_c must be above absolute zero".into());
    }
    if settings.end_s <= 0.0 || settings.output_step_s <= 0.0 {
        return Err("end_s and output_step_s must be positive".into());
    }
    Ok(settings)
}

pub(super) fn path_with_extension(p: &str, extension: &str) -> Result<PathBuf, String> {
    if p.trim().is_empty() {
        return Err("path required".into());
    }
    let mut path = PathBuf::from(p);
    match path.extension() {
        None => {
            path.set_extension(extension);
        }
        Some(value) if value.eq_ignore_ascii_case(extension) => {}
        Some(_) => return Err(format!("expected a .{extension} file")),
    }
    if path.is_relative() {
        path = workdir().join(path);
    }
    Ok(path)
}

pub(super) fn validate_component_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "*"
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>'))
    {
        return Err("use a component name without spaces or angle brackets, and set 表示ラベル for descriptive text".into());
    }
    Ok(())
}

impl Server {
    fn thermal_doc(&mut self) -> Result<&mut Doc, String> {
        self.doc
            .as_ref()
            .ok_or("open a .qucst file or call new_thermal_schematic")?;
        let doc = self.doc()?;
        if doc.sch.version != thermal::VERSION {
            return Err("open a Qucs-T .qucst file for thermal analysis".into());
        }
        Ok(doc)
    }

    fn thermal_results(&mut self) -> Result<&ThermalRun, String> {
        self.thermal_doc()?;
        let doc = self.doc.as_ref().unwrap();
        let run = self
            .thermal_run
            .as_ref()
            .ok_or("run_thermal_analysis first")?;
        if doc.sch != run.snapshot {
            return Err("the thermal results are stale; run_thermal_analysis again".into());
        }
        Ok(run)
    }

    pub(super) fn call_thermal(&mut self, name: &str, a: &Value) -> Result<Vec<Value>, String> {
        let result = match name {
            "new_thermal_schematic" | "generate_thermal_cylinder" => {
                let path = path_with_extension(s(a, "path").ok_or("path required")?, "qucst")?;
                let sch = if name == "generate_thermal_cylinder" {
                    let config: CylinderArgs =
                        serde_json::from_value(a.get("config").cloned().ok_or("config required")?)
                            .map_err(|e| format!("invalid cylinder config: {e}"))?;
                    CylinderGrid {
                        length_mm: config.length_mm,
                        inner_mm: config.inner_mm,
                        axial: config.axial,
                        layers: config.layers,
                        surfaces: [
                            config.surfaces.end_a,
                            config.surfaces.end_b,
                            config.surfaces.inner,
                            config.surfaces.outer,
                        ],
                    }
                    .generate()?
                } else {
                    thermal::empty()
                };
                let mut doc = if b(a, "overwrite").unwrap_or(false) {
                    Doc::new(sch, Some(path.clone()))
                } else {
                    Doc::for_new_file(sch, path.clone())
                };
                doc.save()?;
                let conn = doc.conn();
                let report = crate::check::check(&doc.sch, &conn);
                let surfaces: Vec<_> = conn
                    .nets
                    .iter()
                    .filter(|n| n.named)
                    .map(|n| n.name.as_str())
                    .collect();
                self.doc = Some(doc);
                self.sim = None;
                self.thermal_run = None;
                json!({"path": path, "format": "qucst", "metrics": report.metrics, "named_nets": surfaces})
            }
            "get_thermal_settings" => json!(Settings::read(&self.thermal_doc()?.sch)?),
            "set_thermal_settings" => {
                if a.get("settings").is_none() {
                    return Err("settings required".into());
                }
                let doc = self.thermal_doc()?;
                let settings = settings(&doc.sch, a)?;
                doc.checkpoint();
                settings.store(&mut doc.sch);
                self.commit()?;
                json!(settings)
            }
            "run_thermal_analysis" => {
                let kind = analysis_kind(s(a, "kind").ok_or("kind required")?)?;
                let doc = self.thermal_doc()?;
                let settings = settings(&doc.sch, a)?;
                let conn = doc.conn();
                let network = Network::build(&doc.sch, &conn)?;
                let results = if kind == "steady" {
                    network.steady()?
                } else {
                    network.transient(&settings)?
                };
                let map = TemperatureMap::new(&doc.sch, &conn, &results);
                let run = ThermalRun {
                    kind,
                    snapshot: doc.sch.clone(),
                    results,
                    settings,
                    map,
                };
                let summary = run.summary();
                self.thermal_run = Some(run);
                summary
            }
            "get_thermal_results" => self.thermal_results()?.query(a)?,
            "export_thermal_csv" => {
                let path = path_with_extension(s(a, "path").ok_or("path required")?, "csv")?;
                let run = self.thermal_results()?;
                let csv = run.results.csv();
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
                if let Err(e) = file.write_all(csv.as_bytes()).and_then(|()| file.flush()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(format!("cannot write {}: {e}", path.display()));
                }
                json!({"path":path,"samples":run.results.time_s.len()})
            }
            "run_thermal_sweep" => {
                let doc = self.thermal_doc()?;
                let grid = CylinderGrid::read(&doc.sch)
                    .ok_or("open a schematic generated by generate_thermal_cylinder")?;
                let settings = settings(&doc.sch, a)?;
                let parameter = match s(a, "parameter") {
                    Some("outer_thickness") => SweepParameter::OuterThickness,
                    Some("outer_convection") => SweepParameter::OuterConvection,
                    Some("mesh_multiplier") => SweepParameter::Mesh,
                    _ => {
                        return Err(
                            "choose outer_thickness, outer_convection or mesh_multiplier".into(),
                        )
                    }
                };
                let observe = position(a)?.unwrap_or([
                    grid.length_mm * 0.5,
                    (grid.inner_mm + grid.layers[0].outer_mm) * 0.5,
                ]);
                let sweep = Sweep {
                    parameter,
                    from: number(a, "from")?,
                    to: number(a, "to")?,
                    count: integer(a, "count", 0)?,
                    steady: analysis_kind(s(a, "kind").unwrap_or("steady"))? == "steady",
                    observe_z: observe[0],
                    observe_r: observe[1],
                };
                let rows: Vec<_> = sweep.run(&grid, &settings)?.into_iter().map(|row| {
                    match row.result {
                        Ok(values) => json!({"value":row.value,"cells":row.cells,"peak_temperature_c":values.temperature,
                            "peak_release_w":values.release,"observed_peak_c":values.observed}),
                        Err(error) => json!({"value":row.value,"cells":row.cells,"error":error}),
                    }
                }).collect();
                json!({"parameter":s(a, "parameter"),"kind":if sweep.steady { "steady" } else { "transient" },"position_mm":observe,"rows":rows})
            }
            _ => return Err(format!("unknown thermal tool {name}")),
        };
        Ok(vec![text(&result)])
    }
}

fn analysis_kind(value: &str) -> Result<&'static str, String> {
    match value {
        "steady" => Ok("steady"),
        "transient" => Ok("transient"),
        _ => Err("kind must be steady or transient".into()),
    }
}

fn position(a: &Value) -> Result<Option<[f64; 2]>, String> {
    a.get("position_mm")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| "position_mm must be [axial position, radius] in mm".into())
        })
        .transpose()
}

fn integer(a: &Value, key: &str, default: usize) -> Result<usize, String> {
    a.get(key).map_or(Ok(default), |value| {
        serde_json::from_value(value.clone())
            .map_err(|_| format!("{key} must be a non-negative integer"))
    })
}

fn number(a: &Value, key: &str) -> Result<f64, String> {
    a.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{key} must be a number"))
}

impl ThermalRun {
    fn group(&self, name: &str) -> Result<(&[Series], &str), String> {
        Ok(match name {
            "temperature" => (&self.results.temperatures_c, "°C"),
            "branch_heat" => (&self.results.heat_flow_w, "W"),
            "boundary_heat" => (&self.results.boundary_heat_w, "W"),
            "balance" => (&self.results.balance_w, "W"),
            _ => {
                return Err(format!(
                    "unknown group {name} (choose {})",
                    GROUPS.join(", ")
                ))
            }
        })
    }

    fn summary(&self) -> Value {
        let r = &self.results;
        json!({"kind":self.kind,"settings":self.settings,"samples":r.time_s.len(),
        "end_s":r.time_s.last(),"peak_temperature_c":r.peak_temperature_c,"peak_release_w":r.peak_release_w,
        "cylindrical_positions":self.map.is_some(),"signals":{
            "temperature":r.temperatures_c.len(),"branch_heat":r.heat_flow_w.len(),
            "boundary_heat":r.boundary_heat_w.len(),"balance":r.balance_w.len()
        }})
    }

    fn query(&self, a: &Value) -> Result<Value, String> {
        let location = position(a)?;
        let group_name = s(a, "group").unwrap_or("temperature");
        let (group, unit) = self.group(group_name)?;
        let index = match (location, a.get("signal_index"), s(a, "signal")) {
            (Some([z, r]), None, None) if group_name == "temperature" => Some(
                self.map
                    .as_ref()
                    .ok_or("position_mm requires a generated cylinder")?
                    .signal_at(z, r)
                    .ok_or("no temperature cell at this position")?,
            ),
            (None, Some(_), None) => Some(integer(a, "signal_index", 0)?),
            (None, None, Some(name)) => Some(
                group
                    .iter()
                    .position(|signal| signal.name == name)
                    .ok_or_else(|| format!("no signal named '{name}' in {group_name}"))?,
            ),
            (None, None, None) => None,
            _ => {
                return Err(
                    "choose one of signal, signal_index, or position_mm (temperature only)".into(),
                )
            }
        };
        let offset = integer(a, "offset", 0)?;
        let limit = integer(a, "limit", 100)?;
        if !(1..=1000).contains(&limit) {
            return Err("limit must be between 1 and 1000".into());
        }
        let Some(index) = index else {
            if a.get("at_s").is_some() {
                return Err("select a signal to query at_s".into());
            }
            let signals: Vec<_> = group
                .iter()
                .enumerate()
                .skip(offset)
                .take(limit)
                .map(|(i, signal)| {
                    if self.kind == "steady" {
                        json!({"signal_index":i,"name":signal.name,"value":signal.values[0]})
                    } else {
                        json!({"signal_index":i,"name":signal.name})
                    }
                })
                .collect();
            return Ok(
                json!({"summary":self.summary(),"group":group_name,"unit":unit,"signals":signals,"offset":offset,"total_signals":group.len()}),
            );
        };
        let signal = group.get(index).ok_or("signal_index is out of range")?;
        let time = &self.results.time_s;
        let mut result = json!({"kind":self.kind,"group":group_name,"unit":unit,"signal_index":index,"name":signal.name,"position_mm":location,
            "total_samples":time.len()});
        if group_name == "temperature" && self.kind == "transient" {
            result["peak_c"] = json!(Results::peak(signal));
            result["first_reach_s"] =
                json!(self.results.first_reach(signal, self.settings.threshold_c));
        }
        if let Some(at) = a.get("at_s") {
            let times: Vec<f64> = serde_json::from_value(at.clone())
                .map_err(|_| "at_s must be an array of numbers")?;
            if times.len() > 256 {
                return Err("at_s accepts up to 256 times".into());
            }
            result["samples"] = json!(times.into_iter().map(|at| json!({"time_s":at,"value":crate::plot::sample_at(time, &signal.values, at)})).collect::<Vec<_>>());
        } else {
            result["offset"] = json!(offset);
            result["samples"] = json!(time
                .iter()
                .zip(&signal.values)
                .skip(offset)
                .take(limit)
                .map(|(time, value)| json!({"time_s":time,"value":value}))
                .collect::<Vec<_>>());
        }
        Ok(result)
    }
}
