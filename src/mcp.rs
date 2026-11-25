//! MCP server over stdio (JSON-RPC 2.0, newline delimited).

use crate::check;
use crate::doc::{apply_props, Doc, ExternalChange};
use crate::library::{self, lib};
use crate::render::{build_scene, fit_view, scene_svg, svg_to_png, Decor, Overlay};
use crate::schematic::Schematic;
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::SystemTime;

#[path = "mcp_thermal.rs"]
mod thermal_tools;

const INSTRUCTIONS: &str = "Qucs-T thermal network simulator and schematic editor. Coordinates are Qucs units: x to the right, y down, 10-unit grid. \
Pins are addressed as \"Rth1.1\", points as \"[x,y]\", and nets as \"net:name\". \
Start with new_thermal_schematic, open_schematic (.qucst), or generate_thermal_cylinder. \
Use list_component_types and get_component_type for thermal elements, then add_components, update_component, and connect. \
Use check_layout and snapshot to inspect the layout, and reroute or tidy to adjust it. \
Use run_thermal_analysis for steady and transient analysis, get_thermal_results for signals and samples, or export_thermal_csv. \
Edits are saved automatically. Use open_gui to show the live editor.";

pub struct Server {
    doc: Option<Doc>,
    thermal_run: Option<thermal_tools::ThermalRun>,
    snap_counter: usize,
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": required}})
}

fn tools() -> Vec<Value> {
    let comp_item = json!({
        "type": "object",
        "properties": {
            "type": {"type": "string", "description": "thermal component type, e.g. ThResistance, ThPower, ThBoundary (see list_component_types)"},
            "name": {"type": "string"},
            "x": {"type": "integer"}, "y": {"type": "integer"},
            "rotate": {"type": "integer", "description": "rotation count 0..3 (each 90° counter-clockwise)"},
            "mirror": {"type": "boolean"},
            "props": {"type": "object", "description": "property values by name, e.g. {\"R [K/W]\": \"2\"}; use {\"value\": .., \"show\": true} to toggle display"},
            "align": {"type": "object", "description": "place so that pin `pin` of this part lands at `to` (pin or [x,y]) + (dx,dy). Overrides x/y.",
                "properties": {"pin": {"type": "string", "description": "pin name of this component, e.g. \"1\""}, "to": {"type": "string"}, "dx": {"type": "integer"}, "dy": {"type": "integer"}}}
        },
        "required": ["type"]
    });
    let mut tools = vec![
        tool("new_schematic", "Create a new empty Qucs-T .qucst file (overwrites only if overwrite=true).", json!({"path": {"type": "string"}, "overwrite": {"type": "boolean"}}), &["path"]),
        tool("open_schematic", "Open a Qucs-T .qucst file.", json!({"path": {"type": "string"}}), &["path"]),
        tool("save_schematic", "Save the schematic, optionally at a new path. Edits are saved automatically. To replace a different existing .qucst file pass overwrite=true.", json!({"path": {"type": "string"}, "overwrite": {"type": "boolean"}}), &[]),
        tool("get_schematic", "Components (with absolute pin positions and nets), wires and nets of the open schematic.", json!({"include_wires": {"type": "boolean"}}), &[]),
        tool("list_component_types", "Search thermal component types.", json!({"query": {"type": "string"}, "category": {"type": "string"}}), &[]),
        tool("get_component_type", "Properties (name/default/description), pins and symbol size of a component type.", json!({"type": {"type": "string"}}), &["type"]),
        tool("add_components", "Place components. Returns their pins (absolute coordinates and lead direction) so you can plan wiring.", json!({"components": {"type": "array", "items": comp_item}}), &["components"]),
        tool("update_component", "Change properties, orientation, position or name of a component (wires are NOT moved; use move_components for that).",
            json!({"name": {"type": "string"}, "props": {"type": "object"}, "rotate": {"type": "integer"}, "mirror": {"type": "boolean"}, "x": {"type": "integer"}, "y": {"type": "integer"}, "new_name": {"type": "string"}, "mirror_y": {"type": "boolean", "description": "mirror about the Y axis (left-right)"}, "active": {"type": "integer", "description": "1 active, 0 off"}, "label_offset": {"type": "array", "items": {"type": "integer"}, "description": "[tx, ty] text position relative to the component"}}), &["name"]),
        tool("move_components", "Move components (absolute x/y or relative dx/dy) and re-route the wires attached to them.",
            json!({"moves": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "x": {"type": "integer"}, "y": {"type": "integer"}, "dx": {"type": "integer"}, "dy": {"type": "integer"}}, "required": ["name"]}}, "reroute": {"type": "boolean", "description": "default true"}}), &["moves"]),
        tool("duplicate_components", "Copy components (and the wires between them) with an offset; names are renumbered. Returns the new names.",
            json!({"names": {"type": "array", "items": {"type": "string"}}, "dx": {"type": "integer"}, "dy": {"type": "integer"}}), &["names"]),
        tool("align_components", "Align or distribute components: mode left|right|top|bottom|hcenter|vcenter|distribute_h|distribute_v. Wires are re-routed.",
            json!({"names": {"type": "array", "items": {"type": "string"}}, "mode": {"type": "string"}}), &["names", "mode"]),
        tool("add_painting", "Add a drawing: kind line|rect|ellipse|arrow|text. line/arrow: (x,y)->(x+w,y+h); rect/ellipse: box; text: text at (x,y).",
            json!({"kind": {"type": "string"}, "x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}, "text": {"type": "string"}, "size": {"type": "integer"}, "color": {"type": "string", "description": "#rrggbb"}, "width": {"type": "integer"}, "filled": {"type": "boolean"}}), &["kind", "x", "y"]),
        tool("delete_paintings", "Delete drawings by index (get_schematic lists them) or all.", json!({"indices": {"type": "array", "items": {"type": "integer"}}, "all": {"type": "boolean"}}), &[]),
        tool("export_image", "Write the schematic as PNG or SVG (report style, no overlays) to a file.", json!({"path": {"type": "string"}, "scale": {"type": "number", "description": "pixels per unit, default 2"}}), &["path"]),
        tool("delete_components", "Delete components (and optionally the wires of nets left without pins).", json!({"names": {"type": "array", "items": {"type": "string"}}, "delete_dangling_wires": {"type": "boolean"}}), &["names"]),
        tool("connect", "Auto-route wires. Each entry of `nets` is a list of terminals that become one net (\"Rth1.2\", \"[x,y]\", \"net:name\"). Optional labels give the net a name.",
            json!({"nets": {"type": "array", "items": {"type": "array", "items": {"type": "string"}}}, "labels": {"type": "array", "items": {"type": "string"}, "description": "net label per entry of nets (\"\" for none)"},
                   "improve": {"type": "boolean", "description": "rip-up-and-retry pass to reduce crossings/bends (default true)"}}), &["nets"]),
        tool("add_wire", "Draw a wire through explicit points [[x,y],...] (diagonal steps are split horizontal-first).", json!({"points": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}}}), &["points"]),
        tool("delete_wires", "Delete wires by index, by net name, or all.", json!({"indices": {"type": "array", "items": {"type": "integer"}}, "nets": {"type": "array", "items": {"type": "string"}}, "all": {"type": "boolean"}}), &[]),
        tool("add_net_label", "Name a net (label placed automatically in free space). Same names connect nets, and the label identifies the node in temperature results.", json!({"target": {"type": "string", "description": "pin, [x,y] or net:name"}, "label": {"type": "string"}}), &["target", "label"]),
        tool("reroute", "Rip up and re-route the given nets (by name) or all nets. Use after rearranging parts or when wiring looks messy.", json!({"nets": {"type": "array", "items": {"type": "string"}}}), &[]),
        tool("tidy", "Clean up: snap to grid, merge/remove redundant wires, re-place labels, optionally re-route everything.", json!({"reroute": {"type": "boolean"}, "relabel": {"type": "boolean"}}), &[]),
        tool("check_layout", "Find problems: unconnected pins, pins/wire ends on the middle of wires (look connected but are not), overlapping parts or wires, wires through symbols, overlapping text, off-grid items. Also returns wiring metrics.", json!({}), &[]),
        tool("snapshot", "Render the schematic exactly as the editor draws it (PNG). Overlays help reasoning: coordinate grid+rulers, net colours and names, pin names, numbered issue markers.",
            json!({
                "region": {"type": "array", "items": {"type": "number"}, "description": "[x0,y0,x1,y1] in schematic units"},
                "around": {"type": "array", "items": {"type": "string"}, "description": "zoom to these components"},
                "grid": {"type": "boolean"}, "rulers": {"type": "boolean"}, "nets": {"type": "boolean"}, "pins": {"type": "boolean"}, "issues": {"type": "boolean"},
                "highlight": {"type": "array", "items": {"type": "string"}, "description": "component names or net:name to highlight"},
                "scale": {"type": "number", "description": "pixels per unit (default: fit into max_size)"},
                "max_size": {"type": "integer", "description": "default 1400"},
                "clean": {"type": "boolean", "description": "report style: no grid, rulers or overlays"}
            }), &[]),
        tool("open_gui", "Open the live editor window for the current schematic (it follows every change made here).", json!({}), &[]),
        tool("get_gui_state", "What the user has selected / is looking at in the editor window.", json!({}), &[]),
    ];
    tools.extend(thermal_tools::tools());
    tools
}

fn text(v: &Value) -> Value {
    json!({"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()})
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(|v| v.as_str())
}
fn i(a: &Value, k: &str) -> Option<i32> {
    a.get(k).and_then(|v| v.as_f64()).map(|f| f.round() as i32)
}
fn b(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(|v| v.as_bool())
}
fn strs(a: &Value, k: &str) -> Vec<String> {
    a.get(k).and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}

pub fn workdir() -> PathBuf {
    std::env::var_os("QUCS_MCP_WORKDIR").map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("qucs_work"))
}

fn gui_dir() -> PathBuf {
    std::env::temp_dir().join("qucs-t")
}

pub fn gui_state_path() -> PathBuf {
    gui_dir().join("gui_state.json")
}

pub fn gui_heartbeat_path() -> PathBuf {
    gui_dir().join("gui_alive")
}

pub fn gui_open_request_path() -> PathBuf {
    gui_dir().join("open_request.txt")
}

/// True when an editor window updated its heartbeat within the last 3 seconds.
pub fn gui_running() -> bool {
    std::fs::metadata(gui_heartbeat_path()).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64() < 3.0).unwrap_or(false)
}

impl Server {
    pub fn new() -> Server {
        Server { doc: None, thermal_run: None, snap_counter: 0 }
    }

    fn doc(&mut self) -> Result<&mut Doc, String> {
        // pick up edits saved from the GUI
        if let Some(d) = &self.doc {
            match d.external_change()? {
                Some(ExternalChange::Modified(bytes)) => {
                    let path = d.path.as_deref().expect("external change has a path");
                    self.doc = Some(Doc::from_disk(path, bytes)?);
                }
                Some(ExternalChange::Missing) => return Err("the current schematic was removed; open a schematic to continue".into()),
                None => {}
            }
        }
        self.doc.as_mut().ok_or_else(|| "no schematic open (new_thermal_schematic / open_schematic)".to_string())
    }

    fn commit(&mut self) -> Result<(), String> {
        let d = self.doc.as_mut().ok_or("no schematic")?;
        if d.path.is_some() {
            if let Err(e) = d.save() {
                self.doc = d.last_saved();
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn call(&mut self, name: &str, a: &Value) -> Result<Vec<Value>, String> {
        let result = self.call_impl(name, a);
        if result.is_err() {
            if let Some(doc) = self.doc.as_ref().filter(|d| d.dirty) {
                self.doc = doc.last_saved();
            }
        }
        result
    }

    fn call_impl(&mut self, name: &str, a: &Value) -> Result<Vec<Value>, String> {
        match name {
            "new_thermal_schematic" | "generate_thermal_cylinder" | "get_thermal_settings" | "set_thermal_settings" |
            "run_thermal_analysis" | "get_thermal_results" | "export_thermal_csv" | "run_thermal_sweep" => self.call_thermal(name, a),
            "new_schematic" => self.call_thermal("new_thermal_schematic", a),
            "open_schematic" => {
                let p = thermal_tools::path_with_extension(s(a, "path").ok_or("path required")?, "qucst")?;
                let d = Doc::open(&p)?;
                self.doc = Some(d);
                self.thermal_run = None;
                let info = self.schematic_json(false)?;
                Ok(vec![text(&info)])
            }
            "save_schematic" => {
                if let Some(p) = s(a, "path") {
                    let doc = self.doc()?;
                    let path = thermal_tools::path_with_extension(p, "qucst")?;
                    if !b(a, "overwrite").unwrap_or(false)
                        && !doc.path.as_ref().is_some_and(|old| crate::doc::same_path(old, &path)) {
                        let mut saved = Doc::for_new_file(doc.sch.clone(), path);
                        saved.save()?;
                        self.doc = Some(saved);
                    } else {
                        doc.save_as(path)?;
                    }
                } else {
                    self.doc()?;
                    self.commit()?;
                }
                let p = self.doc()?.path.clone().ok_or("no file name: pass path")?;
                Ok(vec![text(&json!({"saved": p.display().to_string()}))])
            }
            "get_schematic" => Ok(vec![text(&self.schematic_json(b(a, "include_wires").unwrap_or(true))?)]),
            "list_component_types" => {
                let query = s(a, "query").unwrap_or("").to_lowercase();
                let category = s(a, "category").unwrap_or("").to_ascii_lowercase();
                let types: Vec<Value> = lib().comps.iter().filter_map(|c| {
                    let haystack = format!("{} {} {} {}", c.typ, c.caption, c.description, c.category).to_lowercase();
                    (c.category.contains(&category) && query.split_whitespace().all(|word| haystack.contains(word)))
                        .then(|| json!({"type": c.typ, "caption": c.caption, "category": c.category, "pins": c.pin_names}))
                }).collect();
                let categories = (query.is_empty() && category.is_empty()).then(|| json!({"thermal": lib().comps.len()}));
                Ok(vec![text(&json!({"count": types.len(), "types": types, "categories": categories}))])
            }
            "get_component_type" => {
                let t = s(a, "type").ok_or("type required")?;
                let info = lib().info(t).ok_or_else(|| format!("unknown type '{t}'"))?;
                let c = library::new_component(t, "X1", 0, 0)?;
                let p = library::place(&c);
                Ok(vec![text(&json!({
                    "type": info.typ, "caption": info.caption, "description": info.description, "category": info.category,
                    "props": info.props.iter().map(|p| json!({"name": p.name, "default": p.default, "shown": p.display, "description": p.description})).collect::<Vec<_>>(),
                    "default_rotate": c.rotate,
                    "pins_at_origin": p.pins.iter().map(|q| json!({"name": q.name, "x": q.x, "y": q.y, "dir": [q.dir.0, q.dir.1]})).collect::<Vec<_>>(),
                    "body": p.bbox,
                }))])
            }
            "add_components" => {
                let items = a.get("components").and_then(|v| v.as_array()).cloned().ok_or("components required")?;
                let mut added = Vec::new();
                let d = self.doc()?;
                d.checkpoint();
                for it in &items {
                    let typ = s(it, "type").ok_or("each component needs a type")?;
                    if !crate::thermal::is_element(typ) {
                        return Err(format!("unknown thermal component type {typ}"));
                    }
                    if let Some(name) = s(it, "name") {
                        thermal_tools::validate_component_name(name)?;
                    }
                    let props = it.get("props").and_then(|p| p.as_object()).cloned().unwrap_or_default();
                    let (mut x, mut y) = (i(it, "x").unwrap_or(0), i(it, "y").unwrap_or(0));
                    let name = d.add_component(typ, s(it, "name"), x, y, i(it, "rotate"), b(it, "mirror"), &props)?;
                    if let Some(al) = it.get("align") {
                        let conn = d.conn();
                        let pin = s(al, "pin").ok_or("align.pin required")?;
                        let to = s(al, "to").ok_or("align.to required")?;
                        let own = conn.find_pin(&format!("{name}.{pin}")).ok_or_else(|| format!("{name} has no pin '{pin}'"))?.clone();
                        let tgt = d.resolve_terminal(&conn, to)?;
                        x += tgt.x + i(al, "dx").unwrap_or(0) - own.x;
                        y += tgt.y + i(al, "dy").unwrap_or(0) - own.y;
                        let c = d.sch.component_mut(&name).unwrap();
                        c.x = crate::doc::snap(x);
                        c.y = crate::doc::snap(y);
                    }
                    added.push(name);
                }
                let conn = self.doc()?.conn();
                let d = self.doc()?;
                let info: Vec<Value> = added
                    .iter()
                    .map(|n| {
                        let ci = d.sch.component_index(n).unwrap();
                        let c = &d.sch.components[ci];
                        json!({"name": n, "type": c.typ, "x": c.x, "y": c.y, "rotate": c.rotate, "body": conn.placed[ci].bbox.map(|v| v.round()),
                               "pins": conn.pins.iter().filter(|p| p.comp == ci).map(|p| json!({"pin": p.label, "x": p.x, "y": p.y, "dir": [p.dir.0, p.dir.1]})).collect::<Vec<_>>()})
                    })
                    .collect();
                let overlaps: Vec<String> = check::check(&d.sch, &conn).issues.iter().filter(|i| i.kind == "component_overlap").map(|i| i.message.clone()).collect();
                self.commit()?;
                Ok(vec![text(&json!({"added": info, "overlaps": overlaps}))])
            }
            "update_component" => {
                let n = s(a, "name").ok_or("name required")?.to_string();
                let d = self.doc()?;
                d.checkpoint();
                let ci = d.sch.component_index(&n).ok_or_else(|| format!("no component '{n}'"))?;
                if let Some(new_name) = s(a, "new_name") {
                    thermal_tools::validate_component_name(new_name)?;
                    if new_name != n && d.sch.component(new_name).is_some() {
                        return Err(format!("a component named '{new_name}' already exists"));
                    }
                }
                if i(a, "active").is_some_and(|active| active != 0 && active != 1) {
                    return Err("thermal components accept active=0 or active=1".into());
                }
                if let Some(p) = a.get("props").and_then(|v| v.as_object()) {
                    apply_props(&mut d.sch.components[ci], p)?;
                }
                let c = &mut d.sch.components[ci];
                if let Some(r) = i(a, "rotate") {
                    c.rotate = r.rem_euclid(4);
                }
                if let Some(m) = b(a, "mirror") {
                    c.mirror = m as i32;
                }
                if b(a, "mirror_y").unwrap_or(false) {
                    crate::edit::mirror_y(c);
                }
                if let Some(x) = i(a, "x") {
                    c.x = crate::doc::snap(x);
                }
                if let Some(y) = i(a, "y") {
                    c.y = crate::doc::snap(y);
                }
                if let Some(act) = i(a, "active") {
                    c.active = (c.active & !3) | (act & 3);
                }
                if let Some(nn) = s(a, "new_name") {
                    c.name = nn.to_string();
                }
                if let Some(lo) = a.get("label_offset").and_then(|v| v.as_array()) {
                    if lo.len() == 2 {
                        c.tx = lo[0].as_i64().unwrap_or(0) as i32;
                        c.ty = lo[1].as_i64().unwrap_or(0) as i32;
                    }
                } else if a.get("rotate").is_some() || a.get("mirror").is_some() {
                    d.default_label_pos(ci);
                }
                let conn = d.conn();
                let pins: Vec<Value> = conn.pins.iter().filter(|p| p.comp == ci).map(|p| json!({"pin": p.label, "x": p.x, "y": p.y, "net": conn.nets[p.net].name})).collect();
                let c = d.sch.components[ci].serialize();
                self.commit()?;
                Ok(vec![text(&json!({"component": c, "pins": pins}))])
            }
            "move_components" => {
                let moves = a.get("moves").and_then(|v| v.as_array()).cloned().ok_or("moves required")?;
                let d = self.doc()?;
                d.checkpoint();
                let mut mv = Vec::new();
                for m in &moves {
                    let n = s(m, "name").ok_or("move needs name")?;
                    let c = d.sch.component(n).ok_or_else(|| format!("no component '{n}'"))?;
                    let x = i(m, "x").unwrap_or(c.x) + i(m, "dx").unwrap_or(0);
                    let y = i(m, "y").unwrap_or(c.y) + i(m, "dy").unwrap_or(0);
                    mv.push((n.to_string(), x, y));
                }
                let r = d.move_components(&mv, b(a, "reroute").unwrap_or(true))?;
                self.commit()?;
                Ok(vec![text(&r)])
            }
            "duplicate_components" => {
                let names = strs(a, "names");
                let d = self.doc()?;
                let idx: Result<Vec<usize>, String> = names.iter().map(|n| d.sch.component_index(n).ok_or_else(|| format!("no component '{n}'"))).collect();
                let idx = idx?;
                d.checkpoint();
                let (c, w, _) = crate::edit::duplicate(d, &idx, &[], &[], i(a, "dx").unwrap_or(0), i(a, "dy").unwrap_or(120))?;
                let new: Vec<String> = c.iter().map(|&k| d.sch.components[k].name.clone()).collect();
                let overlaps: Vec<String> = check::check(&d.sch, &d.conn()).issues.iter().filter(|i| i.kind == "component_overlap").map(|i| i.message.clone()).collect();
                self.commit()?;
                Ok(vec![text(&json!({"new_components": new, "wires_copied": w.len(), "overlaps": overlaps}))])
            }
            "align_components" => {
                let names = strs(a, "names");
                let mode = s(a, "mode").ok_or("mode required")?.to_string();
                let d = self.doc()?;
                let idx: Result<Vec<usize>, String> = names.iter().map(|n| d.sch.component_index(n).ok_or_else(|| format!("no component '{n}'"))).collect();
                let moves = crate::edit::align(d, &idx?, &mode)?;
                d.checkpoint();
                let r = d.move_components(&moves, true)?;
                self.commit()?;
                Ok(vec![text(&json!({"moves": moves.iter().map(|m| json!({"name": m.0, "x": m.1, "y": m.2})).collect::<Vec<_>>(), "rerouted": r}))])
            }
            "add_painting" => {
                let line = crate::edit::make_painting(
                    s(a, "kind").ok_or("kind required")?,
                    i(a, "x").unwrap_or(0),
                    i(a, "y").unwrap_or(0),
                    i(a, "w").unwrap_or(0),
                    i(a, "h").unwrap_or(0),
                    s(a, "color").unwrap_or(""),
                    i(a, "width").unwrap_or(if s(a, "kind") == Some("text") { 1 } else { 2 }),
                    s(a, "text").unwrap_or(""),
                    i(a, "size").unwrap_or(12),
                    b(a, "filled").unwrap_or(false),
                )?;
                let d = self.doc()?;
                d.checkpoint();
                d.sch.paintings.push(format!("  {line}"));
                let k = d.sch.paintings.len() - 1;
                self.commit()?;
                Ok(vec![text(&json!({"index": k, "painting": line}))])
            }
            "delete_paintings" => {
                let d = self.doc()?;
                d.checkpoint();
                let n0 = d.sch.paintings.len();
                if b(a, "all").unwrap_or(false) {
                    d.sch.paintings.clear();
                } else {
                    let del: std::collections::HashSet<usize> = a.get("indices").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|x| x.as_u64().map(|u| u as usize)).collect()).unwrap_or_default();
                    let mut k = 0;
                    d.sch.paintings.retain(|_| {
                        let keep = !del.contains(&k);
                        k += 1;
                        keep
                    });
                }
                let removed = n0 - d.sch.paintings.len();
                self.commit()?;
                Ok(vec![text(&json!({"deleted": removed}))])
            }
            "export_image" => {
                let mut path = PathBuf::from(s(a, "path").ok_or("path required")?);
                if path.is_relative() {
                    path = workdir().join(path);
                }
                let d = self.doc()?;
                let conn = d.conn();
                let sc = build_scene(&d.sch, &conn, &Overlay::default());
                let view = fit_view(sc.bounds, 30.0, Some(a.get("scale").and_then(|v| v.as_f64()).unwrap_or(2.0)), 8000.0);
                let (svg, _) = scene_svg(&sc, &view, &Decor::default());
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).ok();
                }
                if path.extension().map(|e| e.eq_ignore_ascii_case("svg")).unwrap_or(false) {
                    std::fs::write(&path, svg).map_err(|e| e.to_string())?;
                } else {
                    std::fs::write(&path, svg_to_png(&svg)?).map_err(|e| e.to_string())?;
                }
                Ok(vec![text(&json!({"saved": path.display().to_string(), "size_px": [view.width_px(), view.height_px()]}))])
            }
            "delete_components" => {
                let names = strs(a, "names");
                let d = self.doc()?;
                d.checkpoint();
                let removed = d.delete_components(&names)?;
                let mut dropped = 0;
                if b(a, "delete_dangling_wires").unwrap_or(true) {
                    let conn = d.conn();
                    let dead: std::collections::HashSet<usize> = conn.islands.iter().filter(|isl| isl.pins.is_empty() && isl.labels.is_empty()).flat_map(|isl| isl.wires.clone()).collect();
                    dropped = dead.len();
                    let mut k = 0;
                    d.sch.wires.retain(|_| {
                        let keep = !dead.contains(&k);
                        k += 1;
                        keep
                    });
                }
                self.commit()?;
                Ok(vec![text(&json!({"deleted": removed, "wires_removed": dropped}))])
            }
            "connect" => {
                let nets: Vec<Vec<String>> = a
                    .get("nets")
                    .and_then(|v| v.as_array())
                    .ok_or("nets required")?
                    .iter()
                    .map(|n| n.as_array().map(|x| x.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect()).unwrap_or_default())
                    .collect();
                let labels = strs(a, "labels");
                let d = self.doc()?;
                d.checkpoint();
                // short nets first: they have the fewest alternatives
                let conn = d.conn();
                let mut order: Vec<usize> = (0..nets.len()).collect();
                let hp = |n: &Vec<String>| -> i32 {
                    let pts: Vec<(i32, i32)> = n.iter().filter_map(|t| d.resolve_terminal(&conn, t).ok()).map(|t| (t.x, t.y)).collect();
                    if pts.is_empty() {
                        return 0;
                    }
                    let (x0, x1) = (pts.iter().map(|p| p.0).min().unwrap(), pts.iter().map(|p| p.0).max().unwrap());
                    let (y0, y1) = (pts.iter().map(|p| p.1).min().unwrap(), pts.iter().map(|p| p.1).max().unwrap());
                    x1 - x0 + y1 - y0
                };
                order.sort_by_key(|&k| hp(&nets[k]));
                let mut out = vec![Value::Null; nets.len()];
                for k in order {
                    let l = labels.get(k).filter(|l| !l.is_empty()).map(|s| s.as_str());
                    out[k] = d.connect(&nets[k], l)?;
                }
                let gain = if b(a, "improve").unwrap_or(true) && nets.len() > 1 { d.improve_routes(2) } else { 0 };
                let relabeled = d.fix_label_collisions();
                let conn = d.conn();
                let rep = check::check(&d.sch, &conn);
                self.commit()?;
                Ok(vec![text(&json!({"results": out, "flipped_for_shorter_wires": [], "improvement": gain, "labels_moved": relabeled, "metrics": rep.metrics}))])
            }
            "add_wire" => {
                let pts: Vec<(i32, i32)> = a
                    .get("points")
                    .and_then(|v| v.as_array())
                    .ok_or("points required")?
                    .iter()
                    .filter_map(|p| {
                        let p = p.as_array()?;
                        Some((p.first()?.as_f64()? as i32, p.get(1)?.as_f64()? as i32))
                    })
                    .collect();
                let d = self.doc()?;
                d.checkpoint();
                let n = d.add_wire_path(&pts);
                self.commit()?;
                Ok(vec![text(&json!({"wires_added": n}))])
            }
            "delete_wires" => {
                let d = self.doc()?;
                d.checkpoint();
                let conn = d.conn();
                let mut del: std::collections::HashSet<usize> = a.get("indices").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|x| x.as_u64().map(|u| u as usize)).collect()).unwrap_or_default();
                for n in strs(a, "nets") {
                    let ni = conn.net_by_name(&n).ok_or_else(|| format!("no net '{n}'"))?;
                    del.extend(conn.nets[ni].wires.iter().copied());
                }
                if b(a, "all").unwrap_or(false) {
                    del.extend(0..d.sch.wires.len());
                }
                let mut k = 0;
                d.sch.wires.retain(|_| {
                    let keep = !del.contains(&k);
                    k += 1;
                    keep
                });
                self.commit()?;
                Ok(vec![text(&json!({"deleted": del.len()}))])
            }
            "add_net_label" => {
                let d = self.doc()?;
                d.checkpoint();
                let r = d.add_net_label(s(a, "target").ok_or("target required")?, s(a, "label").ok_or("label required")?)?;
                self.commit()?;
                Ok(vec![text(&r)])
            }
            "reroute" => {
                let d = self.doc()?;
                d.checkpoint();
                let r = d.reroute(&strs(a, "nets"))?;
                let rep = check::check(&d.sch, &d.conn());
                self.commit()?;
                Ok(vec![text(&json!({"result": r, "metrics": rep.metrics}))])
            }
            "tidy" => {
                let d = self.doc()?;
                d.checkpoint();
                let r = d.tidy(b(a, "relabel").unwrap_or(true), b(a, "reroute").unwrap_or(false))?;
                let rep = check::check(&d.sch, &d.conn());
                self.commit()?;
                Ok(vec![text(&json!({"result": r, "metrics": rep.metrics}))])
            }
            "check_layout" => {
                let d = self.doc()?;
                let conn = d.conn();
                let rep = check::check(&d.sch, &conn);
                Ok(vec![text(&json!({"metrics": rep.metrics, "issues": rep.issues.iter().enumerate().map(|(k, i)| i.json(k + 1)).collect::<Vec<_>>()}))])
            }
            "snapshot" => self.snapshot(a),
            "open_gui" => {
                let doc = self.doc()?;
                let p = doc.path.clone().ok_or("save the schematic first")?;
                self.commit()?;
                if gui_running() {
                    // one editor window: open the file in a new tab there
                    use std::io::Write as _;
                    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(gui_open_request_path()).map_err(|e| e.to_string())?;
                    writeln!(f, "{}", p.display()).map_err(|e| e.to_string())?;
                    return Ok(vec![text(&json!({"opened": p.display().to_string(), "note": "opened as a tab in the running editor window; it follows every change made here"}))]);
                }
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("gui").arg(&p).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000 | 0x0000_0200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
                }
                cmd.spawn().map_err(|e| format!("cannot start GUI: {e}"))?;
                Ok(vec![text(&json!({"opened": p.display().to_string(), "note": "the window reloads automatically whenever this server edits the file"}))])
            }
            "get_gui_state" => {
                if !gui_running() {
                    return Err("the editor window is not running (open_gui)".into());
                }
                let t = std::fs::read_to_string(gui_state_path()).map_err(|_| "the editor state is not ready (open_gui)".to_string())?;
                let v: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                Ok(vec![text(&v)])
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    fn snap_path(&mut self, kind: &str) -> Result<PathBuf, String> {
        let d = self.doc()?;
        let p = d.path.clone().unwrap_or_else(|| workdir().join("untitled.qucst"));
        let dir = p.parent().map(|d| d.join("snapshots")).unwrap_or_else(|| gui_dir().join("snapshots"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        self.snap_counter += 1;
        let ts = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Ok(dir.join(format!("{}_{kind}_{}_{:03}.png", p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), ts % 1_000_000, self.snap_counter)))
    }

    fn snapshot(&mut self, a: &Value) -> Result<Vec<Value>, String> {
        let clean = b(a, "clean").unwrap_or(false);
        let d = self.doc()?;
        let conn = d.conn();
        let rep = check::check(&d.sch, &conn);
        let mut ov = Overlay {
            color_nets: b(a, "nets").unwrap_or(!clean),
            net_tags: b(a, "nets").unwrap_or(!clean),
            pin_names: b(a, "pins").unwrap_or(false),
            ..Default::default()
        };
        let show_issues = b(a, "issues").unwrap_or(!clean);
        let shown_issues: Vec<&check::Issue> = if show_issues { rep.issues.iter().filter(|i| i.severity == "error" || i.kind == "unconnected_pin").collect() } else { vec![] };
        ov.markers = shown_issues.iter().map(|i| (i.at.0, i.at.1, i.message.clone())).collect();
        for h in strs(a, "highlight") {
            if let Some(n) = h.strip_prefix("net:") {
                if let Some(ni) = conn.net_by_name(n) {
                    ov.highlight_nets.push(ni);
                }
            } else if let Some(ci) = d.sch.component_index(&h) {
                ov.highlight.push(ci);
            }
        }
        let scene = build_scene(&d.sch, &conn, &ov);
        let bounds = if let Some(r) = a.get("region").and_then(|v| v.as_array()) {
            let v: Vec<f64> = r.iter().filter_map(|x| x.as_f64()).collect();
            if v.len() != 4 {
                return Err("region must be [x0,y0,x1,y1]".into());
            }
            [v[0], v[1], v[2], v[3]]
        } else if let Some(names) = a.get("around").and_then(|v| v.as_array()) {
            let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for n in names.iter().filter_map(|x| x.as_str()) {
                let ci = d.sch.component_index(n).ok_or_else(|| format!("no component '{n}'"))?;
                let p = &conn.placed[ci];
                bb = [bb[0].min(p.bbox[0]), bb[1].min(p.bbox[1]), bb[2].max(p.bbox[2]), bb[3].max(p.bbox[3])];
            }
            [bb[0] - 60.0, bb[1] - 60.0, bb[2] + 60.0, bb[3] + 60.0]
        } else {
            scene.bounds
        };
        let max = i(a, "max_size").unwrap_or(1400).clamp(200, 4000) as f64;
        let view = fit_view(bounds, if a.get("region").is_some() { 0.0 } else { 30.0 }, a.get("scale").and_then(|v| v.as_f64()), max);
        let decor = Decor { grid: b(a, "grid").unwrap_or(!clean), rulers: b(a, "rulers").unwrap_or(!clean), title: None };
        let (svg, (ox, oy)) = scene_svg(&scene, &view, &decor);
        let png = svg_to_png(&svg)?;
        let path = self.snap_path("snap")?;
        std::fs::write(&path, &png).map_err(|e| e.to_string())?;
        let meta = json!({
            "saved": path.display().to_string(),
            "size_px": [view.width_px() as f64 + ox, view.height_px() as f64 + oy],
            "coordinate_mapping": {"px": format!("(x - {}) * {:.4} + {}", view.x0, view.scale, ox), "py": format!("(y - {}) * {:.4} + {}", view.y0, view.scale, oy), "region": [view.x0, view.y0, view.x1, view.y1]},
            "metrics": rep.metrics,
            "markers": shown_issues.iter().enumerate().map(|(k, i)| json!({"n": k + 1, "kind": i.kind, "message": i.message, "at": [i.at.0.round(), i.at.1.round()]})).collect::<Vec<_>>(),
            "legend": "wire colours = nets (names in boxes); red circles = open pins; numbered red rings = issues; blue dots = junctions",
        });
        Ok(vec![json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&png), "mimeType": "image/png"}), text(&meta)])
    }

    fn schematic_json(&mut self, wires: bool) -> Result<Value, String> {
        let d = self.doc()?;
        let conn = d.conn();
        let comps: Vec<Value> = d
            .sch
            .components
            .iter()
            .enumerate()
            .map(|(ci, c)| {
                let count = lib().info(&c.typ).map_or(c.props.len(), |info| info.props.len());
                let props: Map<String, Value> = c.props.iter().take(count).enumerate().map(|(k, p)| (library::prop_name(c, k), json!(p.value))).collect();
                json!({"name": c.name, "type": c.typ, "x": c.x, "y": c.y, "rotate": c.rotate, "mirror": c.mirror != 0, "active": c.active & 3,
                       "props": props, "body": conn.placed[ci].bbox.map(|v| v.round()),
                       "pins": conn.pins.iter().filter(|p| p.comp == ci).map(|p| json!({"pin": p.label, "x": p.x, "y": p.y, "net": conn.nets[p.net].name})).collect::<Vec<_>>()})
            })
            .collect();
        let nets: Vec<Value> = conn.nets.iter().map(|n| json!({"name": n.name, "pins": n.pins.iter().map(|&p| conn.pins[p].label.clone()).collect::<Vec<_>>(), "wires": n.wires.len()})).collect();
        let mut o = json!({"path": d.path.as_ref().map(|p| p.display().to_string()), "format": "qucst", "components": comps, "nets": nets,
            "paintings": d.sch.paintings.iter().enumerate().map(|(k, p)| json!({"i": k, "painting": p.trim()})).collect::<Vec<_>>()});
        if wires {
            o["wires"] = json!(d.sch.wires.iter().enumerate().map(|(k, w)| json!({"i": k, "p": [w.x1, w.y1, w.x2, w.y2], "net": conn.nets[conn.wire_net[k]].name, "label": if w.label.is_empty() { Value::Null } else { json!(w.label) }})).collect::<Vec<_>>());
        }
        Ok(o)
    }
}

pub fn serve() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut server = Server::new();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let r = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}});
                let _ = writeln!(stdout, "{r}");
                let _ = stdout.flush();
                continue;
            }
        };
        let batch = if let Value::Array(a) = msg { a } else { vec![msg] };
        for m in batch {
            if let Some(resp) = handle(&mut server, &m) {
                let _ = writeln!(stdout, "{resp}");
                let _ = stdout.flush();
            }
        }
    }
}

/// Stateless protocol revision: every request carries its version in `params._meta`.
const MODERN_VERSION: &str = "2026-07-28";
/// Session-based revisions served through the `initialize` handshake (newest first).
const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
/// Lifetime of cacheable list results; the tool set never changes while the server runs.
const LIST_TTL_MS: u64 = 3_600_000;

fn supported_versions() -> Vec<&'static str> {
    std::iter::once(MODERN_VERSION).chain(LEGACY_VERSIONS.iter().copied()).collect()
}

fn server_info() -> Value {
    json!({"name": "qucs-t", "version": env!("CARGO_PKG_VERSION")})
}

fn capabilities() -> Value {
    json!({"tools": {"listChanged": false}})
}

type RpcResult = Result<Value, (i64, String, Option<Value>)>;

/// Serves both protocol eras. A request whose `params._meta` names a protocol version is handled
/// statelessly under 2026-07-28 rules; anything else (the `initialize` handshake and requests
/// from clients that negotiated a session-based revision) keeps the legacy semantics.
pub fn handle(server: &mut Server, m: &Value) -> Option<Value> {
    let id = m.get("id").cloned();
    let method = m.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = m.get("params").cloned().unwrap_or(json!({}));
    let id = id?; // notifications get no response
    let requested = params.get("_meta").and_then(|v| v.get(META_VERSION));
    let result = match requested {
        Some(v) => handle_modern(server, method, &params, v),
        None if method == "server/discover" => Err((-32602, format!("missing params._meta[\"{META_VERSION}\"]"), None)),
        None => handle_legacy(server, method, &params),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((c, msg, data)) => {
            let mut e = json!({"code": c, "message": msg});
            if let Some(d) = data {
                e["data"] = d;
            }
            json!({"jsonrpc": "2.0", "id": id, "error": e})
        }
    })
}

fn handle_modern(server: &mut Server, method: &str, params: &Value, requested: &Value) -> RpcResult {
    let Some(ver) = requested.as_str() else {
        return Err((-32602, format!("params._meta[\"{META_VERSION}\"] must be a string"), None));
    };
    if ver != MODERN_VERSION {
        return Err((-32022, "Unsupported protocol version".into(), Some(json!({"supported": supported_versions(), "requested": ver}))));
    }
    let cacheable = |mut r: Value| {
        r["ttlMs"] = json!(LIST_TTL_MS);
        r["cacheScope"] = json!("public");
        r
    };
    let mut r = match method {
        "server/discover" => cacheable(json!({"supportedVersions": supported_versions(), "capabilities": capabilities(), "instructions": INSTRUCTIONS})),
        "tools/list" => cacheable(json!({"tools": tools()})),
        "resources/list" => cacheable(json!({"resources": []})),
        "prompts/list" => cacheable(json!({"prompts": []})),
        "tools/call" => call_tool(server, params)?,
        // `initialize` and `ping` belong to the session-based era only
        _ => return Err((-32601, format!("method not found: {method}"), None)),
    };
    r["resultType"] = json!("complete");
    r["_meta"] = json!({META_SERVER_INFO: server_info()});
    Ok(r)
}

fn handle_legacy(server: &mut Server, method: &str, params: &Value) -> RpcResult {
    match method {
        "initialize" => {
            // answer with the client's version when we implement it, otherwise our newest session-based one
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let ver = LEGACY_VERSIONS.iter().copied().find(|v| *v == asked).unwrap_or(LEGACY_VERSIONS[0]);
            Ok(json!({
                "protocolVersion": ver,
                "capabilities": capabilities(),
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => call_tool(server, params),
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ => Err((-32601, format!("method not found: {method}"), None)),
    }
}

fn call_tool(server: &mut Server, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !tools().iter().any(|t| t["name"] == name.as_str()) {
        return Err((-32602, format!("unknown tool: {name}"), None));
    }
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.call(&name, &args)));
    Ok(match r {
        Ok(Ok(content)) => json!({"content": content, "isError": false}),
        Ok(Err(e)) => json!({"content": [{"type": "text", "text": format!("Error: {e}")}], "isError": true}),
        Err(_) => json!({"content": [{"type": "text", "text": "Error: internal panic while running the tool"}], "isError": true}),
    })
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    fn modern(method: &str, extra: Value) -> Value {
        let mut params = json!({"_meta": {META_VERSION: MODERN_VERSION, "io.modelcontextprotocol/clientCapabilities": {}}});
        if let (Some(p), Value::Object(e)) = (params.as_object_mut(), extra) {
            p.extend(e);
        }
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    }

    #[test]
    fn discover_lists_both_eras() {
        let r = handle(&mut Server::new(), &modern("server/discover", json!({}))).unwrap();
        let res = &r["result"];
        assert_eq!(res["resultType"], "complete");
        assert_eq!(res["supportedVersions"][0], MODERN_VERSION);
        assert!(res["supportedVersions"].as_array().unwrap().iter().any(|v| v == "2025-06-18"));
        assert_eq!(res["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(res["_meta"][META_SERVER_INFO]["name"], "qucs-t");
        assert!(res["ttlMs"].is_u64() && res["cacheScope"] == "public");
    }

    #[test]
    fn modern_list_and_call() {
        let mut s = Server::new();
        let r = handle(&mut s, &modern("tools/list", json!({}))).unwrap();
        assert_eq!(r["result"]["resultType"], "complete");
        assert!(r["result"]["ttlMs"].is_u64());
        assert!(!r["result"]["tools"].as_array().unwrap().is_empty());
        let r = handle(&mut s, &modern("tools/call", json!({"name": "list_component_types", "arguments": {}}))).unwrap();
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["isError"], false);
        assert!(r["result"].get("ttlMs").is_none());
        let r = handle(&mut s, &modern("tools/call", json!({"name": "no_such_tool"}))).unwrap();
        assert_eq!(r["error"]["code"], -32602);
        // removed from the stateless era
        for m in ["ping", "initialize"] {
            assert_eq!(handle(&mut s, &modern(m, json!({}))).unwrap()["error"]["code"], -32601, "{m}");
        }
    }

    #[test]
    fn unsupported_and_missing_versions() {
        let mut s = Server::new();
        let mut m = modern("tools/list", json!({}));
        m["params"]["_meta"][META_VERSION] = json!("1900-01-01");
        let e = &handle(&mut s, &m).unwrap()["error"];
        assert_eq!(e["code"], -32022);
        assert_eq!(e["data"]["requested"], "1900-01-01");
        assert_eq!(e["data"]["supported"][0], MODERN_VERSION);
        let bare = json!({"jsonrpc": "2.0", "id": 2, "method": "server/discover", "params": {}});
        assert_eq!(handle(&mut s, &bare).unwrap()["error"]["code"], -32602);
    }

    #[test]
    fn legacy_handshake_negotiates() {
        let mut s = Server::new();
        let init = |v: &str| json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": v, "capabilities": {}}});
        for (asked, got) in [("2025-06-18", "2025-06-18"), ("2025-11-25", "2025-11-25"), ("2099-01-01", "2025-11-25"), (MODERN_VERSION, "2025-11-25")] {
            let r = handle(&mut s, &init(asked)).unwrap();
            assert_eq!(r["result"]["protocolVersion"], got, "{asked}");
            assert!(r["result"].get("resultType").is_none());
        }
        let ping = json!({"jsonrpc": "2.0", "id": 3, "method": "ping"});
        assert_eq!(handle(&mut s, &ping).unwrap()["result"], json!({}));
        let list = json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"});
        assert!(handle(&mut s, &list).unwrap()["result"].get("ttlMs").is_none());
    }
}
