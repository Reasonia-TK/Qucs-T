//! Desktop editor (egui). It draws the very same scene the MCP snapshots use and
//! follows the .qucst file live, so the user can watch – and correct – the agent.

use crate::check::{self, Issue};
use crate::doc::{same_path, snap, Doc, ExternalChange};
use crate::library::{self, lib};
use crate::netlist::Connectivity;
use crate::render::{build_scene, Color, Overlay, Scene, Shape};
use crate::schematic::Schematic;
use crate::theme::{self, Palette};
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[path = "gui_thermal.rs"]
mod thermal_ui;
use thermal_ui::{Comparison, CylinderDialog, ThermalView};

#[cfg(windows)]
extern "system" {
    fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
    fn FreeConsole() -> i32;
}

/// When started by double-click Windows gives us a console window of our own; drop it.
fn detach_own_console() {
    #[cfg(windows)]
    unsafe {
        let mut ids = [0u32; 4];
        if GetConsoleProcessList(ids.as_mut_ptr(), 4) == 1 {
            FreeConsole();
        }
    }
}

pub fn run_thermal(path: Option<PathBuf>) {
    // one editor window: hand the file to a running editor (opens as a tab)
    if let Some(p) = &path {
        let alive = crate::mcp::gui_running();
        if alive && std::env::var_os("QUCS_RS_NEW_WINDOW").is_none() {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::mcp::gui_open_request_path()) {
                let abs = plain_path(&p.canonicalize().unwrap_or_else(|_| p.clone()));
                if writeln!(f, "{}", abs.display()).is_ok() {
                    return;
                }
            }
        }
    }
    detach_own_console();
    // the window geometry is not remembered: a saved position can point at a
    // monitor that is gone (multi-display setups), leaving the window off screen
    let title = "Qucs-T";
    let vp = egui::ViewportBuilder::default().with_title(title).with_icon(std::sync::Arc::new(app_icon())).with_inner_size([1280.0, 780.0]).with_maximized(true);
    let opts = eframe::NativeOptions { viewport: vp, ..Default::default() };
    let _ = eframe::run_native(title, opts, Box::new(move |cc| Ok(Box::new(App::new(cc, path)))));
}

#[derive(Clone, PartialEq)]
enum Tool {
    Select,
    Wire,
    AutoWire,
    Label,
    Place(String),
    Paint(String),
}

/// What a primary-button drag in the select tool is moving.
enum Grab {
    WireLabel { wire: usize, orig: (i32, i32), start: (f64, f64) },
    CompLabel { comp: usize, orig: (i32, i32), start: (f64, f64) },
    Paintings { orig: Vec<(usize, String)>, start: (i32, i32), moved: (i32, i32) },
    /// a wire segment: `orig` is the wire list before the drag, `pins` the pin points
    Wire { wire: usize, orig: Vec<crate::schematic::Wire>, pins: std::collections::HashSet<(i32, i32)>, start: (i32, i32), moved: (i32, i32) },
    /// a mixed selection (parts + wires + paintings) moved as one block; `orig` is the
    /// schematic before the drag, `pins` the component pins `(component, x, y)` then
    Group { orig: Box<Schematic>, pins: Vec<(usize, i32, i32)>, comps: Vec<usize>, wires: Vec<usize>, paints: Vec<usize>, start: (i32, i32), moved: (i32, i32) },
}

struct Drag {
    start: (i32, i32),
    orig: Vec<(usize, i32, i32)>,
    moved: bool,
}

struct App {
    thermal: ThermalView,
    thermal_comparison: Option<Comparison>,
    cylinder_dialog: CylinderDialog,
    doc: Doc,
    conn: Connectivity,
    scene: Scene,
    gen: u64,
    scene_key: (u64, bool, bool, bool, Vec<usize>, Vec<usize>),
    offset: Vec2,
    zoom: f32,
    need_fit: bool,
    tool: Tool,
    sel: Vec<usize>,
    sel_wires: Vec<usize>,
    drag: Option<Drag>,
    grab: Option<Grab>,
    sel_paint: Vec<usize>,
    paint_start: Option<(i32, i32)>,
    text_prompt: Option<((i32, i32), String, Option<usize>)>,
    box_start: Option<Pos2>,
    wire_pts: Vec<(i32, i32)>,
    autowire_first: Option<String>,
    place_rot: i32,
    color_nets: bool,
    pin_names: bool,
    show_issues: bool,
    show_grid: bool,
    auto_save: bool,
    auto_reroute: bool,
    last_poll: Instant,
    external_change: bool,
    external_review: Option<Result<Doc, String>>,
    status: String,
    issues: Vec<Issue>,
    show_issue_panel: bool,
    issue_focus: Option<(f64, f64)>,
    center_on_issue: bool,
    thermal_panel_visible: bool,
    theme_pref: egui::ThemePreference,
    /// Theme the schematic scene was built for.
    tex_theme: egui::Theme,
    prop_for: Option<usize>,
    property_editing: bool,
    prop_buf: Vec<String>,
    name_buf: String,
    label_prompt: Option<(String, String)>, // (terminal, text)
    cursor: (i32, i32),
    last_state: String,
    last_state_write: Instant,
    icons: std::collections::HashMap<String, (Vec<Shape>, [f64; 4])>,
    // ---- tabs: the active document lives in the fields above, the others here
    tab_id: u64,
    next_tab_id: u64,
    tabs: Vec<Option<TabState>>,
    active: usize,
    last_heartbeat: Instant,
    // ---- preferences, dialogs
    settings: crate::settings::Settings,
    confirm: Option<Confirm>,
    allow_close: bool,
    find_open: bool,
    find_query: String,
    find_focus: bool,
    about_open: bool,
    canvas_rect: Rect,
    shot: Option<Shot>,
}

/// Self-test hook: `QUCS_RS_SHOT=out.png` saves a screenshot of the window and exits.
/// `QUCS_RS_SHOT_ACTIONS` (comma separated) prepares the view first:
/// `thermal-steady`, `thermal-transient`, `find:<text>`, `about`, `quit`,
/// `select:<name>`, `zoom:<factor>`.
struct Shot {
    path: PathBuf,
    actions: Vec<String>,
    frame: u32,
    requested: bool,
}

/// A pending "save changes?" question.
#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    /// closing the tab with this index
    Tab(usize),
    /// closing the window
    Quit,
}

struct TabState {
    tab_id: u64,
    doc: Doc,
    conn: Connectivity,
    scene: Scene,
    gen: u64,
    offset: Vec2,
    zoom: f32,
    need_fit: bool,
    sel: Vec<usize>,
    sel_wires: Vec<usize>,
    sel_paint: Vec<usize>,
    external_change: bool,
    issues: Vec<Issue>,
    thermal: ThermalView,
}

impl TabState {
    fn new(doc: Doc, id: u64) -> TabState {
        let conn = doc.conn();
        TabState {
            tab_id: id,
            doc,
            conn,
            scene: Scene::default(),
            gen: 1,
            offset: Vec2::ZERO,
            zoom: 1.5,
            need_fit: true,
            sel: vec![],
            sel_wires: vec![],
            sel_paint: vec![],
            external_change: false,
            issues: vec![],
            thermal: ThermalView::default(),
        }
    }
}

fn load_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        ("segoe", r"C:\Windows\Fonts\segoeui.ttf", 0u32),
        ("meiryo", r"C:\Windows\Fonts\meiryo.ttc", 0),
        ("msgothic", r"C:\Windows\Fonts\msgothic.ttc", 0),
        ("noto_cjk", "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("noto_cjk2", "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("noto_cjk3", "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("noto_cjk4", "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 0),
        ("droid", "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf", 0),
        ("hiragino", "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
        ("hiragino2", "/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("arialuni", "/Library/Fonts/Arial Unicode.ttf", 0),
    ];
    let mut first = true;
    let mut cjk = false;
    let only = std::env::var("QUCS_RS_FONTS").ok();
    for (name, path, idx) in candidates {
        if let Some(o) = &only {
            if !o.split(',').any(|x| x == name) {
                continue;
            }
        }
        if name != "segoe" && cjk {
            continue;
        }
        if let Ok(bytes) = std::fs::read(path) {
            let mut fd = egui::FontData::from_owned(bytes);
            fd.index = idx;
            fonts.font_data.insert(name.to_string(), std::sync::Arc::new(fd));
            let fam = fonts.families.entry(egui::FontFamily::Proportional).or_default();
            if first && name == "segoe" {
                fam.insert(0, name.to_string());
            } else {
                fam.push(name.to_string());
                fonts.families.entry(egui::FontFamily::Monospace).or_default().push(name.to_string());
                cjk = true;
            }
            first = false;
        }
    }
    ctx.set_fonts(fonts);
}

/// Window / taskbar icon (same drawing as the .exe icon, see build.rs).
fn app_icon() -> egui::IconData {
    egui::IconData { rgba: crate::icon_pixels::icon_rgba(64), width: 64, height: 64 }
}

/// `canonicalize` on Windows yields `\?\C:\...`; Use plain paths for display and file handoff.
fn plain_path(p: &std::path::Path) -> PathBuf {
    let s = p.display().to_string();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}


fn paint_shapes_xf(painter: &egui::Painter, shapes: &[Shape], alpha: f32, z: f32, xf: &dyn Fn(f64, f64) -> Pos2) {
    let pal: &Palette = theme::pal(painter.ctx());
    let col = |c: &Color| pal.map(c, alpha);
    for s in shapes {
        match s {
            Shape::Line { a, b, color, width, dash } => {
                let pa = xf(a.0, a.1);
                let pb = xf(b.0, b.1);
                let st = Stroke::new((*width as f32 * z).max(1.0), col(color));
                if *dash {
                    painter.extend(egui::Shape::dashed_line(&[pa, pb], st, 4.0 * z, 3.0 * z));
                } else {
                    painter.line_segment([pa, pb], st);
                }
            }
            Shape::Poly { pts, color, width, fill, closed, .. } => {
                let p: Vec<Pos2> = pts.iter().map(|q| xf(q.0, q.1)).collect();
                let st = if *width > 0.0 { Stroke::new((*width as f32 * z).max(1.0), col(color)) } else { Stroke::NONE };
                if let Some(f) = fill {
                    painter.add(egui::Shape::convex_polygon(p.clone(), col(f), Stroke::NONE));
                }
                if *width > 0.0 {
                    if *closed {
                        painter.add(egui::Shape::closed_line(p, st));
                    } else {
                        painter.add(egui::Shape::line(p, st));
                    }
                }
            }
            Shape::Ellipse { c, rx, ry, color, width, fill } => {
                let pc = xf(c.0, c.1);
                let r = Vec2::new(*rx as f32 * z, *ry as f32 * z);
                if let Some(f) = fill {
                    painter.add(egui::Shape::ellipse_filled(pc, r, col(f)));
                }
                if *width > 0.0 {
                    painter.add(egui::Shape::ellipse_stroke(pc, r, Stroke::new((*width as f32 * z).max(1.0), col(color))));
                }
            }
            Shape::Text { pos, text, size, color, bold, angle, bg } => {
                let fs = (*size as f32 * z).max(1.0);
                if fs < 3.0 {
                    continue;
                }
                let p = xf(pos.0, pos.1);
                let galley = painter.layout_no_wrap(text.clone(), FontId::proportional(fs), col(color));
                if let Some(b) = bg {
                    painter.rect_filled(Rect::from_min_size(p - Vec2::new(2.0, 0.0), galley.size() + Vec2::new(4.0, 0.0)), 2.0, col(b));
                }
                let mut ts = egui::epaint::TextShape::new(p, galley.clone(), col(color));
                if angle.abs() > 0.01 {
                    ts = ts.with_angle(angle.to_radians() as f32);
                }
                painter.add(ts);
                if *bold {
                    // faux bold
                    let mut t2 = egui::epaint::TextShape::new(p + Vec2::new(0.6, 0.0), galley, col(color));
                    if angle.abs() > 0.01 {
                        t2 = t2.with_angle(angle.to_radians() as f32);
                    }
                    painter.add(t2);
                }
            }
        }
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> App {
        if std::env::var_os("QUCS_RS_NO_FONTS").is_none() {
            load_fonts(&cc.egui_ctx);
        }
        let theme_pref = theme::load_pref();
        theme::install(&cc.egui_ctx, theme_pref);
        // Ctrl +/- zoom the schematic, not the whole UI
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let settings = crate::settings::Settings::load();
        let mut status = String::new();
        let doc = match path.as_deref().map(Doc::open) {
            Some(Ok(doc)) => doc,
            result => {
                if let Some(Err(error)) = result {
                    status = error;
                }
                Doc::new(crate::thermal::empty(), None)
            }
        };
        let conn = doc.conn();
        let mut settings = settings;
        if let Some(p) = doc.path.as_ref().filter(|p| p.exists()) {
            settings.add_recent(p);
            settings.save();
        }
        let issues = if settings.show_issues { check::check(&doc.sch, &conn).issues } else { vec![] };
        App {
            thermal: ThermalView::default(),
            thermal_comparison: None,
            cylinder_dialog: CylinderDialog::default(),
            conn,
            doc,
            scene: Scene::default(),
            gen: 1,
            scene_key: (0, false, false, false, vec![], vec![]),
            offset: Vec2::ZERO,
            zoom: 1.5,
            need_fit: true,
            tool: Tool::Select,
            sel: vec![],
            sel_wires: vec![],
            drag: None,
            grab: None,
            sel_paint: vec![],
            paint_start: None,
            text_prompt: None,
            box_start: None,
            wire_pts: vec![],
            autowire_first: None,
            place_rot: 0,
            color_nets: settings.color_nets,
            pin_names: settings.pin_names,
            show_issues: settings.show_issues,
            show_grid: settings.show_grid,
            auto_save: settings.auto_save,
            auto_reroute: settings.auto_reroute,
            last_poll: Instant::now(),
            external_change: false,
            external_review: None,
            status: if status.is_empty() { "準備完了".into() } else { status },
            issues,
            show_issue_panel: false,
            issue_focus: None,
            center_on_issue: false,
            thermal_panel_visible: false,
            theme_pref,
            tex_theme: cc.egui_ctx.theme(),
            prop_for: None,
            property_editing: false,
            prop_buf: vec![],
            name_buf: String::new(),
            label_prompt: None,
            cursor: (0, 0),
            last_state: String::new(),
            last_state_write: Instant::now(),
            icons: Default::default(),
            tab_id: 1,
            next_tab_id: 2,
            tabs: vec![None],
            active: 0,
            last_heartbeat: Instant::now() - std::time::Duration::from_secs(10),
            settings,
            confirm: None,
            allow_close: false,
            find_open: false,
            find_query: String::new(),
            find_focus: false,
            about_open: false,
            canvas_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            shot: std::env::var_os("QUCS_RS_SHOT").map(|p| Shot {
                path: PathBuf::from(p),
                actions: std::env::var("QUCS_RS_SHOT_ACTIONS").unwrap_or_default().split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
                frame: 0,
                requested: false,
            }),
        }
    }

    fn drive_shot(&mut self, ctx: &egui::Context) {
        let Some(sh) = self.shot.as_mut() else { return };
        sh.frame += 1;
        let frame = sh.frame;
        ctx.request_repaint();
        if frame == 5 {
            let actions = sh.actions.clone();
            for a in actions {
                let (k, v) = a.split_once(':').unwrap_or((a.as_str(), ""));
                match k {
                    "thermal-steady" => self.thermal_run(true, ctx),
                    "thermal-transient" => self.thermal_run(false, ctx),
                    "find" => {
                        self.find_open = true;
                        self.find_query = v.to_string();
                    }
                    "about" => self.about_open = true,
                    "quit" => {
                        self.doc.dirty = true;
                        self.auto_save = false;
                        self.confirm = Some(Confirm::Quit);
                    }
                    "select" => self.sel = self.doc.sch.component_index(v).into_iter().collect(),
                    "zoom" => self.zoom_by(v.parse().unwrap_or(1.0)),
                    _ => {}
                }
            }
        }
        // screenshot once background work is done
        let busy = self.thermal.busy();
        if let Some(sh) = self.shot.as_mut() {
            if frame > 30 && !busy && !sh.requested {
                sh.requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            if frame > 3000 {
                eprintln!("shot: timed out");
                std::process::exit(2);
            }
        }
        let img = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let (Some(img), Some(sh)) = (img, self.shot.as_ref()) {
            let [w, h] = img.size;
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let r = resvg::tiny_skia::IntSize::from_wh(w as u32, h as u32)
                .and_then(|sz| resvg::tiny_skia::Pixmap::from_vec(rgba, sz))
                .ok_or_else(|| "bad image".to_string())
                .and_then(|pm| pm.encode_png().map_err(|e| e.to_string()))
                .and_then(|png| std::fs::write(&sh.path, png).map_err(|e| e.to_string()));
            match r {
                Ok(_) => eprintln!("shot: {} ({}) {}", sh.path.display(), self.status, w),
                Err(e) => eprintln!("shot failed: {e}"),
            }
            self.shot = None;
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Auto-save may be blocked by a conflict or a write error.
    fn unsaved(&self, d: &Doc) -> bool {
        d.dirty
    }

    fn tab_unsaved(&self, i: usize) -> bool {
        if i == self.active {
            self.unsaved(&self.doc) || self.buffered_property_edit().is_some()
        } else {
            self.tabs.get(i).and_then(|t| t.as_ref()).map(|t| self.unsaved(&t.doc)).unwrap_or(false)
        }
    }

    /// Save the active document (asks for a name when it has none). False when not saved.
    fn save_current(&mut self) -> bool {
        if self.doc.path.is_none() {
            return self.save_as();
        }
        self.finish_buffered_property_edit();
        match self.doc.save() {
            Ok(p) => {
                self.external_change = false;
                self.external_review = None;
                self.status = format!("保存: {}", p.display());
                true
            }
            Err(e) => {
                self.report_save_error(e);
                false
            }
        }
    }

    /// Write the view toggles into the settings and store them.
    fn store_settings(&mut self) {
        let s = &mut self.settings;
        s.color_nets = self.color_nets;
        s.pin_names = self.pin_names;
        s.show_issues = self.show_issues;
        s.show_grid = self.show_grid;
        s.auto_save = self.auto_save;
        s.auto_reroute = self.auto_reroute;
        s.save();
    }

    // ------------------------------------------------------------------ model sync

    fn changed(&mut self) {
        self.refresh_after_edit();
        self.auto_save_current();
    }

    fn auto_save_current(&mut self) {
        if self.auto_save && self.doc.path.is_some() {
            if let Err(e) = self.doc.save() {
                self.report_save_error(e);
            }
        }
    }

    fn refresh_after_edit(&mut self) {
        self.gen += 1;
        self.conn = self.doc.conn();
        self.sel.retain(|&i| i < self.doc.sch.components.len());
        self.sel_wires.retain(|&i| i < self.doc.sch.wires.len());
        self.prop_for = None;
        self.property_editing = false;
        if self.show_issues || self.show_issue_panel {
            self.refresh_issues();
        }
    }

    fn buffered_property_edit(&self) -> Option<usize> {
        let index = self.prop_for?;
        let component = self.doc.sch.components.get(index)?;
        let changed_name = !self.name_buf.is_empty() && self.name_buf != component.name;
        let changed_value = component.props.iter().zip(&self.prop_buf).any(|(prop, value)| prop.value != *value);
        (changed_name || changed_value).then_some(index)
    }

    fn finish_buffered_property_edit(&mut self) -> bool {
        let Some(index) = self.buffered_property_edit() else { return false };
        self.doc.checkpoint();
        let component = &mut self.doc.sch.components[index];
        if !self.name_buf.is_empty() {
            component.name.clone_from(&self.name_buf);
        }
        for (prop, value) in component.props.iter_mut().zip(&self.prop_buf) {
            prop.value.clone_from(value);
        }
        self.refresh_after_edit();
        true
    }

    fn report_save_error(&mut self, error: String) {
        if matches!(self.doc.external_change(), Ok(Some(_))) {
            self.external_change = true;
            self.status = "外部変更があるため保存を保留しました".into();
        } else {
            self.status = format!("保存エラー: {error}");
        }
    }

    fn install_external(&mut self, doc: Doc) {
        self.need_fit |= self.doc.sch.components.is_empty();
        self.doc = doc;
        self.gen += 1;
        self.conn = self.doc.conn();
        self.sel.clear();
        self.sel_wires.clear();
        self.sel_paint.clear();
        self.drag = None;
        self.grab = None;
        self.box_start = None;
        self.wire_pts.clear();
        self.autowire_first = None;
        self.prop_for = None;
        self.property_editing = false;
        if self.show_issues || self.show_issue_panel {
            self.refresh_issues();
        }
        self.external_change = false;
        self.external_review = None;
        self.status = "ファイルの外部変更を反映しました".into();
    }

    fn poll_disk(&mut self) {
        if self.last_poll.elapsed().as_millis() < 300 {
            return;
        }
        self.last_poll = Instant::now();
        let Some(path) = self.doc.path.as_deref() else { return };
        match self.doc.external_change() {
            Ok(None) => {
                self.external_change = false;
                self.external_review = None;
            }
            Ok(Some(ExternalChange::Modified(bytes))) if !self.doc.dirty && !self.has_pending_edits() => {
                match Doc::from_disk(path, bytes) {
                    Ok(doc) => self.install_external(doc),
                    Err(_) => self.external_change = true,
                }
            }
            Ok(Some(_)) | Err(_) => self.external_change = true,
        }
    }

    fn has_pending_edits(&self) -> bool {
        self.property_editing
            || self.buffered_property_edit().is_some()
            || self.drag.is_some()
            || self.grab.is_some()
            || self.paint_start.is_some()
            || !self.wire_pts.is_empty()
            || self.autowire_first.is_some()
            || self.text_prompt.is_some()
            || self.label_prompt.is_some()
            || self.external_review.is_some()
    }

    fn external_change_dialog(&mut self, ctx: &egui::Context) {
        let Some(review) = self.external_review.as_ref() else { return };
        let mut recover = false;
        let mut save_as = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("external-change-review")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("外部変更を確認");
            ui.label("元のファイルへの保存を保留しています。ローカル版を別の回路図として保存できます。");
            match review {
                Ok(external) => {
                    ui.separator();
                    ui.label("現在のローカル版と外部版の違い");
                    let local = &self.doc.sch;
                    let disk = &external.sch;
                    let local_parts: std::collections::HashMap<_, _> = local.components.iter().map(|part| (part.name.as_str(), part)).collect();
                    let external_names: std::collections::HashSet<_> = disk.components.iter().map(|part| part.name.as_str()).collect();
                    let added = disk.components.iter().filter(|part| !local_parts.contains_key(part.name.as_str())).map(|part| part.name.as_str());
                    let changed = disk.components.iter().filter(|part| local_parts.get(part.name.as_str()).is_some_and(|old| *old != *part)).map(|part| part.name.as_str());
                    let removed = local.components.iter().filter(|part| !external_names.contains(part.name.as_str())).map(|part| part.name.as_str());
                    for (label, names) in [("追加された部品", added.collect::<Vec<_>>()), ("変更された部品", changed.collect()), ("削除された部品", removed.collect())] {
                        if !names.is_empty() {
                            let summary = names[..names.len().min(8)].join("、");
                            let more = if names.len() > 8 { format!(" ほか{}個", names.len() - 8) } else { String::new() };
                            ui.label(format!("{label}　{summary}{more}"));
                        }
                    }
                    if local.wires != disk.wires {
                        ui.label(format!("配線が変更されています（{}本 → {}本）", local.wires.len(), disk.wires.len()));
                    }
                    if local.properties != disk.properties || local.symbol != disk.symbol || local.diagrams != disk.diagrams || local.paintings != disk.paintings || local.extra_sections != disk.extra_sections {
                        ui.label("回路図の設定・表示項目にも差があります。");
                    }
                    ui.small("退避ファイルは元の回路図と同じフォルダに作成され、別のタブで開きます。");
                }
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, "外部版を読み込めません。");
                    ui.small(error);
                }
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                recover = ui.add_enabled(review.is_ok(), egui::Button::new("ローカル版を退避して外部版を開く")).clicked();
                save_as = ui.button("ローカル版を別名で保存…").clicked();
                close = ui.button("編集に戻る").clicked();
            });
        });
        if recover {
            self.recover_external_change();
        } else if save_as {
            self.save_as();
        } else if close || modal.should_close() {
            self.external_review = None;
        }
    }

    fn recover_external_change(&mut self) {
        let Some(Ok(external)) = self.external_review.take() else { return };
        match external.external_change() {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => {
                self.external_review = self.doc.path.as_deref().map(Doc::open);
                self.status = "確認中に外部版が更新されました。変更内容をもう一度確認してください".into();
                return;
            }
        }
        self.finish_buffered_property_edit();
        match self.doc.save_conflict_draft() {
            Ok(draft) => {
                let original_tab = self.active;
                self.install_external(external);
                self.open_path(&draft);
                self.switch_tab(original_tab);
                self.status = format!("外部版を開きました。ローカル版の退避先: {}", draft.display());
            }
            Err(e) => {
                self.status = format!("ローカル版を退避できません: {e}");
                self.external_review = Some(Ok(external));
            }
        }
    }

    fn write_state(&mut self, canvas: Rect) {
        if self.last_state_write.elapsed().as_millis() < 400 {
            return;
        }
        self.last_state_write = Instant::now();
        let tl = self.to_sch(canvas, canvas.min);
        let br = self.to_sch(canvas, canvas.max);
        let st = serde_json::json!({
            "file": self.doc.path.as_ref().map(|p| p.display().to_string()),
            "format": "qucst",
            "selected_components": self.sel.iter().filter_map(|&i| self.doc.sch.components.get(i).map(|c| c.name.clone())).collect::<Vec<_>>(),
            "selected_wires": self.sel_wires.iter().filter_map(|&i| self.doc.sch.wires.get(i).map(|w| serde_json::json!({"i": i, "p": [w.x1, w.y1, w.x2, w.y2]}))).collect::<Vec<_>>(),
            "visible_region": [tl.0, tl.1, br.0, br.1],
            "cursor": [self.cursor.0, self.cursor.1],
            "tool": match &self.tool { Tool::Select => "select".to_string(), Tool::Wire => "wire".into(), Tool::AutoWire => "autowire".into(), Tool::Label => "label".into(), Tool::Place(t) => format!("place:{t}"), Tool::Paint(k) => format!("paint:{k}") },
            "unsaved_changes": self.doc.dirty,
            "open_tabs": (0..self.tabs.len()).map(|i| if i == self.active { self.doc.path.clone() } else { self.tabs[i].as_ref().and_then(|t| t.doc.path.clone()) }.map(|p| p.display().to_string())).collect::<Vec<_>>(),
        })
        .to_string();
        if st != self.last_state {
            let p = crate::mcp::gui_state_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&p, &st);
            self.last_state = st;
        }
    }

    // ------------------------------------------------------------------ coordinates

    fn to_screen(&self, canvas: Rect, x: f64, y: f64) -> Pos2 {
        canvas.min + (Vec2::new(x as f32, y as f32) - self.offset) * self.zoom
    }
    fn to_sch_f(&self, canvas: Rect, p: Pos2) -> (f64, f64) {
        let v = (p - canvas.min) / self.zoom + self.offset;
        (v.x as f64, v.y as f64)
    }
    fn to_sch(&self, canvas: Rect, p: Pos2) -> (i32, i32) {
        let (x, y) = self.to_sch_f(canvas, p);
        (x.round() as i32, y.round() as i32)
    }
    fn snapped(&self, canvas: Rect, p: Pos2) -> (i32, i32) {
        let (x, y) = self.to_sch(canvas, p);
        (snap(x), snap(y))
    }

    fn fit(&mut self, canvas: Rect) {
        let b = self.scene.bounds;
        let (w, h) = ((b[2] - b[0]).max(100.0) as f32 + 80.0, (b[3] - b[1]).max(100.0) as f32 + 80.0);
        self.zoom = (canvas.width() / w).min(canvas.height() / h).clamp(0.2, 4.0);
        let cx = (b[0] + b[2]) as f32 / 2.0;
        let cy = (b[1] + b[3]) as f32 / 2.0;
        self.offset = Vec2::new(cx, cy) - canvas.size() / 2.0 / self.zoom;
    }

    // ------------------------------------------------------------------ hit tests

    fn comp_at(&self, x: f64, y: f64) -> Option<usize> {
        let mut best: Option<(f64, usize)> = None;
        for (i, p) in self.conn.placed.iter().enumerate() {
            let b = p.bbox;
            if x >= b[0] - 2.0 && x <= b[2] + 2.0 && y >= b[1] - 2.0 && y <= b[3] + 2.0 {
                let area = (b[2] - b[0]) * (b[3] - b[1]);
                if best.map(|bb| area < bb.0).unwrap_or(true) {
                    best = Some((area, i));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Start dragging the whole selection (parts, wires and paintings) as one block.
    fn start_group(&mut self, px: f64, py: f64) {
        let pins = self.conn.pins.iter().map(|p| (p.comp, p.x, p.y)).collect();
        self.grab = Some(Grab::Group {
            orig: Box::new(self.doc.sch.clone()),
            pins,
            comps: self.sel.clone(),
            wires: self.sel_wires.clone(),
            paints: self.sel_paint.clone(),
            start: (px.round() as i32, py.round() as i32),
            moved: (0, 0),
        });
    }

    fn wire_at(&self, x: f64, y: f64) -> Option<usize> {
        let tol = (5.0 / self.zoom as f64).max(2.0);
        for (i, w) in self.doc.sch.wires.iter().enumerate() {
            let (x1, y1, x2, y2) = (w.x1 as f64, w.y1 as f64, w.x2 as f64, w.y2 as f64);
            let (dx, dy) = (x2 - x1, y2 - y1);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - x1) * dx + (y - y1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (x1 + t * dx, y1 + t * dy);
            if ((px - x).powi(2) + (py - y).powi(2)).sqrt() <= tol {
                return Some(i);
            }
        }
        None
    }

    fn pin_at(&self, x: f64, y: f64) -> Option<String> {
        let tol = (8.0 / self.zoom as f64).max(4.0);
        self.conn
            .pins
            .iter()
            .map(|p| (((p.x as f64 - x).powi(2) + (p.y as f64 - y).powi(2)).sqrt(), p))
            .filter(|(d, _)| *d <= tol)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p.label.clone())
    }

    // ------------------------------------------------------------------ actions

    fn rotate_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.doc.checkpoint();
        for &i in &self.sel.clone() {
            let c = &mut self.doc.sch.components[i];
            c.rotate = (c.rotate + 1) % 4;
            self.doc.default_label_pos(i);
        }
        self.reroute_selected();
        self.changed();
    }

    fn mirror_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.doc.checkpoint();
        for &i in &self.sel.clone() {
            let c = &mut self.doc.sch.components[i];
            c.mirror = 1 - c.mirror.clamp(0, 1);
        }
        self.reroute_selected();
        self.changed();
    }

    fn reroute_selected(&mut self) {
        if !self.auto_reroute {
            return;
        }
        let moves: Vec<(String, i32, i32)> = self.sel.iter().map(|&i| {
            let c = &self.doc.sch.components[i];
            (c.name.clone(), c.x, c.y)
        }).collect();
        if let Err(e) = self.doc.move_components(&moves, true) {
            self.status = e;
        }
    }

    fn copy_selection(&mut self, ctx: &egui::Context, cut: bool) {
        if self.sel.is_empty() && self.sel_wires.is_empty() && self.sel_paint.is_empty() {
            return;
        }
        let text = crate::edit::copy_text(&self.doc, &self.sel, &self.sel_wires, &self.sel_paint);
        ctx.copy_text(text);
        self.status = format!("{} 個の部品をコピー", self.sel.len());
        if cut {
            self.delete_selected();
        }
    }

    fn paste(&mut self, text: &str) {
        // place the clip so that its top-left lands at the cursor
        let Ok(clip) = Schematic::parse(text.trim()) else {
            self.status = "クリップボードに回路図データがありません".into();
            return;
        };
        if clip.components.iter().any(|c| !crate::thermal::is_element(&c.typ)) {
            self.status = "熱回路素子だけ貼り付けられます".into();
            return;
        }
        let mut minx = i32::MAX;
        let mut miny = i32::MAX;
        for c in &clip.components {
            minx = minx.min(c.x);
            miny = miny.min(c.y);
        }
        for w in &clip.wires {
            minx = minx.min(w.x1.min(w.x2));
            miny = miny.min(w.y1.min(w.y2));
        }
        if minx == i32::MAX {
            minx = self.cursor.0;
            miny = self.cursor.1;
        }
        self.doc.checkpoint();
        match crate::edit::paste_text(&mut self.doc, text, snap(self.cursor.0 - minx), snap(self.cursor.1 - miny)) {
            Ok((c, w, p)) => {
                self.status = format!("{} 個の部品を貼り付け", c.len());
                self.changed();
                self.sel = c;
                self.sel_wires = w;
                self.sel_paint = p;
            }
            Err(e) => self.status = e,
        }
    }

    fn duplicate_selection(&mut self) {
        if self.sel.is_empty() && self.sel_paint.is_empty() {
            return;
        }
        self.doc.checkpoint();
        match crate::edit::duplicate(&mut self.doc, &self.sel.clone(), &self.sel_wires.clone(), &self.sel_paint.clone(), 40, 40) {
            Ok((c, w, p)) => {
                self.changed();
                self.sel = c;
                self.sel_wires = w;
                self.sel_paint = p;
                self.status = "複製しました".into();
            }
            Err(e) => self.status = e,
        }
    }

    fn mirror_y_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.doc.checkpoint();
        for &i in &self.sel.clone() {
            crate::edit::mirror_y(&mut self.doc.sch.components[i]);
            self.doc.default_label_pos(i);
        }
        self.reroute_selected();
        self.changed();
    }

    fn align_selected(&mut self, mode: &str) {
        match crate::edit::align(&self.doc, &self.sel, mode) {
            Ok(moves) => {
                self.doc.checkpoint();
                if let Err(e) = self.doc.move_components(&moves, self.auto_reroute) {
                    self.status = e;
                }
                self.changed();
            }
            Err(e) => self.status = e,
        }
    }

    fn export_image(&mut self) {
        let Some(p) = rfd::FileDialog::new().add_filter("PNG", &["png"]).add_filter("SVG", &["svg"]).set_file_name("schematic.png").save_file() else { return };
        let ov = Overlay::default();
        let sc = build_scene(&self.doc.sch, &self.conn, &ov);
        let view = crate::render::fit_view(sc.bounds, 30.0, Some(2.0), 6000.0);
        let (svg, _) = crate::render::scene_svg(&sc, &view, &crate::render::Decor::default());
        let res = if p.extension().map(|e| e.eq_ignore_ascii_case("svg")).unwrap_or(false) {
            std::fs::write(&p, svg).map_err(|e| e.to_string())
        } else {
            crate::render::svg_to_png(&svg).and_then(|png| std::fs::write(&p, png).map_err(|e| e.to_string()))
        };
        self.status = match res {
            Ok(_) => format!("書き出し: {}", p.display()),
            Err(e) => format!("書き出し失敗: {e}"),
        };
    }

    fn delete_selected(&mut self) {
        if self.sel.is_empty() && self.sel_wires.is_empty() && self.sel_paint.is_empty() {
            return;
        }
        self.doc.checkpoint();
        let mut ps = std::mem::take(&mut self.sel_paint);
        ps.sort_unstable();
        for &k in ps.iter().rev() {
            if k < self.doc.sch.paintings.len() {
                self.doc.sch.paintings.remove(k);
            }
        }
        let names: Vec<String> = self.sel.iter().map(|&i| self.doc.sch.components[i].name.clone()).collect();
        let _ = self.doc.delete_components(&names);
        let mut ws = self.sel_wires.clone();
        ws.sort_unstable();
        for &w in ws.iter().rev() {
            if w < self.doc.sch.wires.len() {
                self.doc.sch.wires.remove(w);
            }
        }
        self.sel.clear();
        self.sel_wires.clear();
        self.changed();
    }

    /// Input for operating point and Monte Carlo. Keep edits off disk except for the
    /// calculation's temporary copy, which lives beside the original for relative references.
    fn blank_schematic(&self) -> Schematic {
        crate::thermal::empty()
    }

    fn file_type(&self) -> (&'static str, &'static str) {
        ("Qucs-T 熱回路", "qucst")
    }

    fn open_dialog(&mut self) {
        let (description, extension) = self.file_type();
        if let Some(files) = rfd::FileDialog::new().add_filter(description, &[extension]).pick_files() {
            for p in files {
                self.open_path(&p);
            }
        }
    }

    /// Open a file in a tab (switches to it when it is already open).
    fn open_path(&mut self, p: &std::path::Path) {
        let canon = |x: &std::path::Path| x.canonicalize().unwrap_or_else(|_| x.to_path_buf());
        let want = canon(p);
        if self.doc.path.as_deref().map(canon) == Some(want.clone()) {
            return;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.as_ref().and_then(|t| t.doc.path.as_deref().map(canon)) == Some(want.clone())) {
            self.switch_tab(i);
            return;
        }
        let result = Doc::open(p);
        match result {
            Ok(d) => {
                // reuse an empty, untitled tab
                if self.doc.path.is_none() && self.doc.sch.components.is_empty() && self.doc.sch.wires.is_empty() && !self.doc.dirty {
                    let id = self.tab_id;
                    self.install_tab(TabState::new(d, id));
                } else {
                    self.new_tab(d);
                }
                self.status = format!("開きました: {}", p.display());
                self.settings.add_recent(p);
                self.settings.save();
            }
            Err(e) => {
                self.status = e;
                // drop stale entries from the recent list
                if !p.exists() {
                    self.settings.recent.retain(|r| std::path::Path::new(r) != p);
                    self.settings.save();
                }
            }
        }
    }

    // ------------------------------------------------------------------ DC operating point

    fn export_pdf(&mut self) {
        let name = self.doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| format!("{}.pdf", s.to_string_lossy())).unwrap_or_else(|| "schematic.pdf".into());
        let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name).save_file() else { return };
        let title = self.doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.status = match crate::pdf::schematic_pdf(&self.doc.sch, &self.conn, &title).and_then(|b| std::fs::write(&p, b).map_err(|e| e.to_string())) {
            Ok(_) => format!("PDF 書き出し: {}", p.display()),
            Err(e) => format!("PDF 書き出し失敗: {e}"),
        };
    }

    /// Print = PDF in the temp folder, opened in the system viewer.
    fn print(&mut self) {
        let title = self.doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "untitled".into());
        let p = std::env::temp_dir().join("qucs-t").join(format!("{title}_print.pdf"));
        let r = crate::pdf::schematic_pdf(&self.doc.sch, &self.conn, &title).and_then(|b| {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            std::fs::write(&p, b).map_err(|e| e.to_string())
        });
        self.status = match r.and_then(|_| crate::pdf::open_with_system(&p)) {
            Ok(_) => "印刷用 PDF をビューアで開きました。ビューアから印刷してください".into(),
            Err(e) => format!("印刷の準備に失敗: {e}"),
        };
    }

    // ------------------------------------------------------------------ hierarchy

    fn find_window(&mut self, ctx: &egui::Context) {
        if !self.find_open {
            return;
        }
        let mut open = true;
        let mut goto: Option<(Vec<usize>, Vec<usize>, (f64, f64))> = None;
        egui::Window::new("検索").open(&mut open).default_size([320.0, 360.0]).show(ctx, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.find_query).hint_text("部品名・種類・値・ネット名").desired_width(f32::INFINITY));
            if self.find_focus {
                r.request_focus();
                self.find_focus = false;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let q = self.find_query.trim().to_lowercase();
            if q.is_empty() {
                ui.small("入力すると一致する部品とネットを表示します。クリックで選択して移動。");
                return;
            }
            let mut hits: Vec<(String, Vec<usize>, Vec<usize>, (f64, f64))> = Vec::new();
            for (i, c) in self.doc.sch.components.iter().enumerate() {
                let props: Vec<&str> = c.props.iter().map(|p| p.value.as_str()).collect();
                let hay = format!("{} {} {}", c.name, c.typ, props.join(" ")).to_lowercase();
                if hay.contains(&q) {
                    let b = self.conn.placed.get(i).map(|p| p.bbox).unwrap_or([c.x as f64; 4]);
                    let val = c.props.first().map(|p| p.value.clone()).unwrap_or_default();
                    hits.push((format!("{}  ({} {})", c.name, c.typ, val), vec![i], vec![], ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)));
                }
            }
            for net in &self.conn.nets {
                if net.name.to_lowercase().contains(&q) {
                    let at = net.wires.first().and_then(|&w| self.doc.sch.wires.get(w)).map(|w| (w.x1 as f64, w.y1 as f64)).or_else(|| net.pins.first().map(|&p| (self.conn.pins[p].x as f64, self.conn.pins[p].y as f64)));
                    if let Some(at) = at {
                        hits.push((format!("ネット {}  ({} ピン)", net.name, net.pins.len()), vec![], net.wires.clone(), at));
                    }
                }
            }
            ui.small(format!("{} 件", hits.len()));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (k, (label, comps, wires, at)) in hits.iter().enumerate() {
                    if ui.selectable_label(false, label).clicked() || (enter && k == 0) {
                        goto = Some((comps.clone(), wires.clone(), *at));
                    }
                }
            });
        });
        self.find_open = open;
        if let Some((comps, wires, (x, y))) = goto {
            self.tool = Tool::Select;
            self.sel = comps;
            self.sel_wires = wires;
            self.sel_paint.clear();
            let c = self.canvas_rect;
            self.offset = Vec2::new(x as f32, y as f32) - c.size() / 2.0 / self.zoom;
        }
    }

    fn zoom_by(&mut self, factor: f32) {
        let c = self.canvas_rect;
        let center = c.center();
        let before = self.to_sch_f(c, center);
        self.zoom = (self.zoom * factor).clamp(0.1, 12.0);
        let after = self.to_sch_f(c, center);
        self.offset += Vec2::new((before.0 - after.0) as f32, (before.1 - after.1) as f32);
    }

    // ------------------------------------------------------------------ dialogs

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Qucs-T について").open(&mut open).collapsible(false).show(ctx, |ui| {
            ui.heading(format!("Qucs-T {}", env!("CARGO_PKG_VERSION")));
            ui.label("熱回路網シミュレーター");
            ui.label("Qucs-S-Rust の回路図エディタをベースに開発");
            ui.label("GPL-2.0-or-later");
        });
        self.about_open = open;
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = self.confirm else { return };
        enum Answer {
            Save,
            Discard,
            Cancel,
        }
        let mut answer = None;
        let unsaved: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tab_unsaved(i)).collect();
        egui::Window::new("変更の保存").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            match c {
                Confirm::Tab(i) => {
                    ui.label(format!("「{}」への変更を保存しますか？", self.tab_title(i).trim_end_matches(" *")));
                }
                Confirm::Quit => {
                    ui.label("保存していない変更があります:");
                    for &i in &unsaved {
                        ui.label(format!("  • {}", self.tab_title(i).trim_end_matches(" *")));
                    }
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let save = if c == Confirm::Quit { "すべて保存して終了" } else { "保存" };
                let discard = if c == Confirm::Quit { "保存せずに終了" } else { "保存しない" };
                if ui.button(save).clicked() {
                    answer = Some(Answer::Save);
                }
                if ui.button(discard).clicked() {
                    answer = Some(Answer::Discard);
                }
                if ui.button("キャンセル").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(Answer::Cancel);
                }
            });
        });
        let Some(a) = answer else { return };
        self.confirm = None;
        match (c, a) {
            (_, Answer::Cancel) => {}
            (Confirm::Tab(i), Answer::Discard) => self.force_close_tab(i),
            (Confirm::Tab(i), Answer::Save) => {
                self.switch_tab(i);
                if self.save_current() {
                    self.force_close_tab(self.active);
                }
            }
            (Confirm::Quit, Answer::Discard) => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            (Confirm::Quit, Answer::Save) => {
                for i in unsaved {
                    self.switch_tab(i);
                    if !self.save_current() {
                        self.status = "保存されなかったため終了を中止しました".into();
                        return;
                    }
                }
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    // ------------------------------------------------------------------ tabs

    fn stash_tab(&mut self) -> TabState {
        if self.finish_buffered_property_edit() {
            self.auto_save_current();
        }
        TabState {
            tab_id: self.tab_id,
            doc: std::mem::replace(&mut self.doc, Doc::new(Schematic::default(), None)),
            conn: std::mem::replace(&mut self.conn, Connectivity::build(&Schematic::default())),
            scene: std::mem::take(&mut self.scene),
            gen: self.gen,
            offset: self.offset,
            zoom: self.zoom,
            need_fit: self.need_fit,
            sel: std::mem::take(&mut self.sel),
            sel_wires: std::mem::take(&mut self.sel_wires),
            sel_paint: std::mem::take(&mut self.sel_paint),
            external_change: self.external_change,
            issues: std::mem::take(&mut self.issues),
            thermal: std::mem::take(&mut self.thermal),
        }
    }

    fn install_tab(&mut self, t: TabState) {
        self.tab_id = t.tab_id;
        self.doc = t.doc;
        self.conn = t.conn;
        self.scene = t.scene;
        self.gen = t.gen + 1;
        // the cached scene key belongs to the previous tab and its gen can equal ours
        // (every new tab starts at gen 2), so invalidate it: gen is never 0
        self.scene_key.0 = 0;
        self.offset = t.offset;
        self.zoom = t.zoom;
        self.need_fit = t.need_fit;
        self.sel = t.sel;
        self.sel_wires = t.sel_wires;
        self.sel_paint = t.sel_paint;
        self.external_change = t.external_change;
        self.external_review = None;
        self.issues = t.issues;
        self.thermal = t.thermal;
        // transient interaction state belongs to no tab
        self.drag = None;
        self.grab = None;
        self.box_start = None;
        self.wire_pts.clear();
        self.autowire_first = None;
        self.paint_start = None;
        self.prop_for = None;
        self.property_editing = false;
        self.last_poll = Instant::now() - std::time::Duration::from_secs(1);
        self.issue_focus = None;
        self.center_on_issue = false;
        if self.show_issues || self.show_issue_panel {
            self.refresh_issues();
        }
    }

    fn new_tab(&mut self, d: Doc) {
        let cur = self.stash_tab();
        self.tabs[self.active] = Some(cur);
        self.tabs.push(None);
        self.active = self.tabs.len() - 1;
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.install_tab(TabState::new(d, id));
    }

    fn switch_tab(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() {
            return;
        }
        let cur = self.stash_tab();
        self.tabs[self.active] = Some(cur);
        let t = self.tabs[i].take().expect("inactive tab state");
        self.active = i;
        self.install_tab(t);
        self.poll_disk();
    }

    /// Close a tab; asks first when it has unsaved changes.
    fn close_tab(&mut self, i: usize) {
        if i < self.tabs.len() && self.tab_unsaved(i) {
            self.confirm = Some(Confirm::Tab(i));
            return;
        }
        self.force_close_tab(i);
    }

    fn force_close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            let id = self.next_tab_id;
            self.next_tab_id += 1;
            self.install_tab(TabState::new(Doc::new(self.blank_schematic(), None), id));
            return;
        }
        if i == self.active {
            let next = if i + 1 < self.tabs.len() { i + 1 } else { i - 1 };
            let t = self.tabs[next].take().expect("inactive tab state");
            self.tabs.remove(i);
            self.active = if next > i { next - 1 } else { next };
            self.install_tab(t);
        } else {
            self.tabs.remove(i);
            if i < self.active {
                self.active -= 1;
            }
        }
    }

    fn tab_title(&self, i: usize) -> String {
        let (path, dirty) = if i == self.active {
            (self.doc.path.clone(), self.unsaved(&self.doc))
        } else {
            match &self.tabs[i] {
                Some(t) => (t.doc.path.clone(), self.unsaved(&t.doc)),
                None => (None, false),
            }
        };
        let name = path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "無題".into());
        format!("{name}{}", if dirty { " *" } else { "" })
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut switch = None;
        let mut close = None;
        ui.horizontal_wrapped(|ui| {
            for i in 0..self.tabs.len() {
                let title = self.tab_title(i);
                let full = if i == self.active { self.doc.path.clone() } else { self.tabs[i].as_ref().and_then(|t| t.doc.path.clone()) };
                let r = ui.selectable_label(i == self.active, &title).on_hover_text(full.map(|p| p.display().to_string()).unwrap_or_default());
                if r.clicked() {
                    switch = Some(i);
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                if ui.small_button("✖").on_hover_text("タブを閉じる (Ctrl+W)").clicked() {
                    close = Some(i);
                }
                ui.separator();
            }
            if ui.small_button("＋").on_hover_text("新しいタブ (Ctrl+T)").clicked() {
                self.new_tab(Doc::new(self.blank_schematic(), None));
            }
        });
        if let Some(i) = close {
            self.close_tab(i);
        } else if let Some(i) = switch {
            self.switch_tab(i);
        }
    }

    // ------------------------------------------------------------------ single instance

    /// Tell MCP servers that this editor is alive (they then send files here instead of starting another window).
    fn heartbeat(&mut self) {
        if self.last_heartbeat.elapsed().as_millis() < 1000 {
            return;
        }
        self.last_heartbeat = Instant::now();
        let p = crate::mcp::gui_heartbeat_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, std::process::id().to_string());
    }

    fn poll_open_requests(&mut self) {
        let p = crate::mcp::gui_open_request_path();
        let Ok(text) = std::fs::read_to_string(&p) else { return };
        let _ = std::fs::remove_file(&p);
        for line in text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()) {
            self.open_path(std::path::Path::new(line));
        }
    }

    // ------------------------------------------------------------------ Monte Carlo

    fn save_as(&mut self) -> bool {
        let (description, extension) = self.file_type();
        let name = self.doc.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| format!("untitled.{extension}"));
        let Some(mut p) = rfd::FileDialog::new().add_filter(description, &[extension]).set_file_name(name).save_file() else { return false };
        if p.extension().is_none() {
            p.set_extension(extension);
        }
        if self.tabs.iter().flatten().any(|t| t.doc.path.as_ref().is_some_and(|path| same_path(path, &p))) {
            self.status = "別のタブですでに開いている回路図です。別のファイル名を指定してください".into();
            return false;
        }
        self.finish_buffered_property_edit();
        match self.doc.save_as(p.clone()) {
            Ok(_) => {
                self.external_change = false;
                self.external_review = None;
                self.conn = self.doc.conn();
                self.gen += 1;
                self.status = format!("保存: {}", p.display());
                self.settings.add_recent(&p);
                self.settings.save();
                true
            }
            Err(e) => {
                self.report_save_error(e);
                false
            }
        }
    }

    // ------------------------------------------------------------------ painting

    fn rebuild_scene(&mut self) {
        let key = (self.gen, self.color_nets, self.pin_names, self.show_issues, self.sel.clone(), self.sel_wires.clone());
        if key == self.scene_key {
            return;
        }
        let ov = Overlay {
            color_nets: self.color_nets,
            net_tags: self.color_nets,
            pin_names: self.pin_names,
            selected: self.sel.clone(),
            selected_wires: self.sel_wires.clone(),
            markers: if self.show_issues {
                self.issues.iter().filter(|i| i.severity == "error").map(|i| (i.at.0, i.at.1, i.summary_ja.clone())).collect()
            } else {
                vec![]
            },
            wire_color: Some(theme::to_color(self.pal().wire)),
            ..Default::default()
        };
        self.scene = build_scene(&self.doc.sch, &self.conn, &ov);
        self.scene_key = key;
    }

    fn pal(&self) -> &'static Palette {
        match self.tex_theme {
            egui::Theme::Dark => &theme::DARK,
            egui::Theme::Light => &theme::LIGHT,
        }
    }

    fn paint_shapes(&self, painter: &egui::Painter, canvas: Rect, shapes: &[Shape], alpha: f32) {
        paint_shapes_xf(painter, shapes, alpha, self.zoom, &|x, y| self.to_screen(canvas, x, y));
    }

    fn paint_grid(&self, painter: &egui::Painter, canvas: Rect) {
        let step = if self.zoom * 10.0 >= 7.0 { 10.0 } else if self.zoom * 50.0 >= 7.0 { 50.0 } else { 100.0 };
        let (x0, y0) = self.to_sch_f(canvas, canvas.min);
        let (x1, y1) = self.to_sch_f(canvas, canvas.max);
        let gx0 = (x0 / step).floor() as i64;
        let gx1 = (x1 / step).ceil() as i64;
        let gy0 = (y0 / step).floor() as i64;
        let gy1 = (y1 / step).ceil() as i64;
        if (gx1 - gx0) * (gy1 - gy0) > 40_000 {
            return;
        }
        let c = self.pal().grid;
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                let p = self.to_screen(canvas, gx as f64 * step, gy as f64 * step);
                painter.rect_filled(Rect::from_center_size(p, Vec2::splat(1.4)), 0.0, c);
            }
        }
    }

    // ------------------------------------------------------------------ UI parts

    fn theme_menu(&mut self, ui: &mut egui::Ui) {
        let cur = match self.theme_pref {
            egui::ThemePreference::Dark => "☾ ダーク",
            egui::ThemePreference::Light => "☀ ライト",
            egui::ThemePreference::System => "◐ システム",
        };
        ui.menu_button(cur, |ui| {
            for (p, label) in [
                (egui::ThemePreference::Light, "☀ ライト（アイボリー）"),
                (egui::ThemePreference::Dark, "☾ ダーク（オブシディアン・ゴールド）"),
                (egui::ThemePreference::System, "◐ システム設定に合わせる"),
            ] {
                if ui.selectable_label(self.theme_pref == p, label).clicked() {
                    self.theme_pref = p;
                    ui.ctx().set_theme(p);
                    theme::save_pref(p);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("表示テーマ");
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("📂 開く").on_hover_text("Ctrl+O").clicked() {
                self.open_dialog();
            }
            let recent = self.settings.existing_recent();
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button("🕘 最近", |ui| {
                    for p in &recent {
                        let label = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.button(label).on_hover_text(p.display().to_string()).clicked() {
                            self.open_path(p);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("履歴を消去").clicked() {
                        self.settings.recent.clear();
                        self.settings.save();
                        ui.close();
                    }
                })
                .response
                .on_hover_text("最近使ったファイル");
            });
            if ui.button("💾 保存").on_hover_text("Ctrl+S").clicked() {
                self.save_current();
            }
            if ui.button("名前を付けて保存").on_hover_text("Ctrl+Shift+S").clicked() {
                self.save_as();
            }
            ui.separator();
            if ui.button("↶").on_hover_text("元に戻す (Ctrl+Z)").clicked() && self.doc.undo() {
                self.changed();
            }
            if ui.button("↷").on_hover_text("やり直し (Ctrl+Y)").clicked() && self.doc.redo() {
                self.changed();
            }
            ui.separator();
            for (t, label, tip) in [
                (Tool::Select, "⬉ 選択", "選択・移動 (Esc)"),
                (Tool::Wire, "╱ 配線", "クリックで折れ点、ダブルクリック/右クリックで終了 (W)"),
                (Tool::AutoWire, "⚡ 自動配線", "ピンを2つクリックするとルーターが配線 (A)"),
                (Tool::Label, "🏷 ラベル", "配線/ピンをクリックしてネット名を付ける (L)"),
            ] {
                if ui.selectable_label(self.tool == t, label).on_hover_text(tip).clicked() {
                    self.tool = t;
                    self.wire_pts.clear();
                    self.autowire_first = None;
                }
            }
            ui.separator();
            if ui.button("⟲ 全再配線").on_hover_text("全ネットを引き直す (交差・折れを最小化)").clicked() {
                self.doc.checkpoint();
                match self.doc.reroute(&[]) {
                    Ok(r) => self.status = format!("再配線: {r}"),
                    Err(e) => self.status = e,
                }
                self.changed();
            }
            if ui.button("✨ 整頓").on_hover_text("グリッド吸着・配線結合・ラベル再配置").clicked() {
                self.doc.checkpoint();
                match self.doc.tidy(true, false) {
                    Ok(r) => self.status = format!("整頓: {r}"),
                    Err(e) => self.status = e,
                }
                self.changed();
            }
            if ui.button("🔍 チェック").clicked() {
                self.show_issue_panel = true;
                self.refresh_issues();
                self.gen += 1;
            }
            if ui.button("熱解析…").on_hover_text("F5").clicked() {
                self.open_thermal();
            }
            if ui.button("サンプル").clicked() {
                self.open_thermal_sample();
            }
            if ui.button("円筒回路を生成…").clicked() {
                self.open_cylinder_window();
            }
            ui.menu_button("✏ 図形", |ui| {
                for (k, label) in [("line", "線"), ("rect", "矩形"), ("ellipse", "楕円"), ("arrow", "矢印"), ("text", "テキスト (T)")] {
                    if ui.button(label).clicked() {
                        self.tool = Tool::Paint(k.into());
                        ui.close();
                    }
                }
            });
            ui.menu_button("✂ 編集", |ui| {
                if ui.button("複製 (Ctrl+D)").clicked() {
                    self.duplicate_selection();
                    ui.close();
                }
                if ui.button("Y軸ミラー (Y)").clicked() {
                    self.mirror_y_selected();
                    ui.close();
                }
                if ui.button("検索… (Ctrl+F)").clicked() {
                    self.find_open = true;
                    self.find_focus = true;
                    ui.close();
                }
                ui.separator();
                for (m, label) in [("left", "左揃え"), ("right", "右揃え"), ("top", "上揃え"), ("bottom", "下揃え"), ("vcenter", "縦中心揃え"), ("hcenter", "横中心揃え"), ("distribute_h", "横に等間隔"), ("distribute_v", "縦に等間隔")] {
                    if ui.add_enabled(self.sel.len() >= 2, egui::Button::new(label)).clicked() {
                        self.align_selected(m);
                        ui.close();
                    }
                }
            });
            ui.menu_button("🖼 書き出し", |ui| {
                if ui.button("PNG / SVG…").clicked() {
                    self.export_image();
                    ui.close();
                }
                if ui.button("PDF…").clicked() {
                    self.export_pdf();
                    ui.close();
                }
                if ui.button("印刷… (Ctrl+P)").on_hover_text("PDF を作成して既定のビューアで開きます（ビューアから印刷）").clicked() {
                    self.print();
                    ui.close();
                }
            });
            if ui.button("⛶ 全体表示").on_hover_text("F").clicked() {
                self.need_fit = true;
            }
            ui.separator();
            let mut ch = false;
            ch |= ui.checkbox(&mut self.color_nets, "ネット色").changed();
            ch |= ui.checkbox(&mut self.pin_names, "ピン名").changed();
            ch |= ui.checkbox(&mut self.show_issues, "問題表示").changed();
            let mut other = ui.checkbox(&mut self.show_grid, "グリッド").changed();
            other |= ui.checkbox(&mut self.auto_reroute, "移動時に再配線").changed();
            other |= ui.checkbox(&mut self.auto_save, "自動保存").on_hover_text("回路図の編集を保存します").changed();
            if ch && self.show_issues {
                self.refresh_issues();
            }
            if ch || other {
                self.store_settings();
            }
        });
    }

    /// Symbol drawing of a component type at the origin (cached).
    fn icon(&mut self, typ: &str) -> &(Vec<Shape>, [f64; 4]) {
        self.icons.entry(typ.to_string()).or_insert_with(|| {
            let mut shapes = Vec::new();
            let mut bb = [-20.0, -20.0, 20.0, 20.0];
            if let Ok(c) = library::new_component(typ, "?", 0, 0) {
                let p = library::place(&c);
                for prim in &p.prims {
                    crate::render::prim_shapes(prim, &mut shapes, None);
                }
                bb = p.bbox;
                for pin in &p.pins {
                    bb = [bb[0].min(pin.x as f64), bb[1].min(pin.y as f64), bb[2].max(pin.x as f64), bb[3].max(pin.y as f64)];
                }
            }
            (shapes, bb)
        })
    }

    fn palette_entry(&mut self, ui: &mut egui::Ui, typ: &str, caption: &str, sub_line: &str, description: &str) {
        const ICON: Vec2 = Vec2::new(46.0, 34.0);
        let width = ui.available_width().min(ui.clip_rect().right() - ui.cursor().left()).max(0.0);
        let text = ui.painter().layout(
            format!("{caption}\n{sub_line}"),
            FontId::proportional(12.5),
            ui.visuals().text_color(),
            (width - ICON.x - 12.0).max(16.0),
        );
        // One allocation keeps the scroll area within the palette, even if a label or star
        // needs more room than its assigned rectangle.
        let (_, row) = ui.allocate_space(Vec2::new(width, ICON.y.max(text.size().y) + 4.0));
        if !ui.is_rect_visible(row) {
            return;
        }
        let id = ui.make_persistent_id(typ);
        let selected = matches!(&self.tool, Tool::Place(key) if key == typ);
        let part = ui.interact(row, id, Sense::click());
        part.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, caption));
        let part_visuals = ui.style().interact_selectable(&part, selected);
        let painter = ui.painter().with_clip_rect(row);
        if selected || part.hovered() || part.has_focus() {
            painter.rect_filled(row, 3.0, part_visuals.weak_bg_fill);
        }
        let icon_rect = Rect::from_center_size(Pos2::new(row.left() + 2.0 + ICON.x / 2.0, row.center().y), ICON);
        let (shapes, bounds) = self.icon(typ).clone();
        let (symbol_w, symbol_h) = ((bounds[2] - bounds[0]).max(1.0), (bounds[3] - bounds[1]).max(1.0));
        let scale = ((f64::from(ICON.x) - 4.0) / symbol_w).min((f64::from(ICON.y) - 4.0) / symbol_h).min(0.8);
        let (cx, cy) = ((bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0);
        let center = icon_rect.center();
        let icon_painter = painter.with_clip_rect(icon_rect);
        paint_shapes_xf(&icon_painter, &shapes, 1.0, scale as f32, &|x, y| center + Vec2::new(((x - cx) * scale) as f32, ((y - cy) * scale) as f32));
        let text_pos = Pos2::new(icon_rect.right() + 6.0, row.center().y - text.size().y / 2.0);
        painter.galley(text_pos, text, part_visuals.text_color());
        if part.on_hover_text(description).clicked() {
            self.place_rot = library::new_component(typ, "?", 0, 0).map(|c| c.rotate).unwrap_or(0);
            self.tool = Tool::Place(typ.to_string());
        }
    }

    fn properties(&mut self, ui: &mut egui::Ui) {
        self.property_editing = false;
        if self.sel.len() != 1 {
            ui.heading("プロパティ");
            if self.sel.is_empty() {
                ui.label("部品を選択すると編集できます。");
            } else {
                ui.label(format!("{} 個選択中", self.sel.len()));
                ui.horizontal_wrapped(|ui| {
                    for (m, label) in [("left", "⇤ 左"), ("right", "⇥ 右"), ("top", "⤒ 上"), ("bottom", "⤓ 下"), ("vcenter", "縦中心"), ("hcenter", "横中心"), ("distribute_h", "横等間隔"), ("distribute_v", "縦等間隔")] {
                        if ui.button(label).clicked() {
                            self.align_selected(m);
                        }
                    }
                });
            }
            ui.separator();
            ui.label("ショートカット");
            ui.small("Ctrl+C/X/V/D コピー/切り取り/貼り付け/複製\nCtrl+A 全選択 / Y Y軸ミラー / T テキスト\nR 回転 / M ミラー / Del 削除\nW 配線 / A 自動配線 / L ラベル\nCtrl+Z/Y 元に戻す/やり直し\nCtrl+S 保存 / Ctrl+Shift+S 名前を付けて保存\nCtrl+F 検索 / Ctrl+P 印刷 / F5 解析\nホイール・Ctrl +/-: ズーム, 中/右ドラッグ: パン\nF・Ctrl+0 全体表示 / F1 バージョン情報");
            return;
        }
        let ci = self.sel[0];
        if self.prop_for != Some(ci) {
            self.prop_for = Some(ci);
            self.prop_buf = self.doc.sch.components[ci].props.iter().map(|p| p.value.clone()).collect();
            self.name_buf = self.doc.sch.components[ci].name.clone();
        }
        let mut c = self.doc.sch.components[ci].clone();
        let info = lib().info(&c.typ);
        if let Some(info) = info.filter(|_| matches!(c.typ.as_str(), "ThCapacity" | "ThBoundary")) {
            library::fill_missing_properties(&mut c, info);
            self.prop_buf.extend(c.props.iter().skip(self.prop_buf.len()).map(|p| p.value.clone()));
        }
        ui.heading(format!("{} ({})", c.name, info.map_or(c.typ.as_str(), |i| i.caption.as_str())));
        if let Some(i) = info {
            ui.small(&i.description);
        }
        ui.horizontal(|ui| {
            ui.label("名前");
            let r = ui.text_edit_singleline(&mut self.name_buf);
            self.property_editing |= r.has_focus();
            if r.lost_focus() && self.name_buf != c.name && !self.name_buf.is_empty() {
                self.doc.checkpoint();
                self.doc.sch.components[ci].name = self.name_buf.clone();
                self.changed();
            }
        });
        ui.label(format!("位置 ({}, {})  回転 {}  ミラー {}", c.x, c.y, c.rotate, c.mirror));
        ui.horizontal(|ui| {
            if ui.button("⟲ 回転 (R)").clicked() {
                self.rotate_selected();
            }
            if ui.button("⇅ ミラー (M)").clicked() {
                self.mirror_selected();
            }
            let mut on = c.active & 3 == 1;
            if ui.checkbox(&mut on, "有効").changed() {
                self.doc.checkpoint();
                let cc = &mut self.doc.sch.components[ci];
                cc.active = (cc.active & !3) | if on { 1 } else { 0 };
                self.changed();
            }
        });
        ui.separator();
        let mut commit: Option<(usize, String)> = None;
        let mut toggle: Option<usize> = None;
        let fields = crate::thermal::property_fields(&c);
        egui::ScrollArea::vertical().id_salt("props").show(ui, |ui| {
            // One row per property: name | value | "show" checkbox. The value field takes whatever width is left after
            // the checkbox (right-to-left layout), so it follows the panel when it is dragged wider or narrower and can
            // never push the panel open on its own.
            let body = egui::TextStyle::Body.resolve(ui.style());
            let label_w = (0..c.props.len())
                .filter(|k| fields.is_none_or(|visible| visible.contains(k)))
                .map(|k| ui.painter().layout_no_wrap(library::prop_name(&c, k), body.clone(), egui::Color32::PLACEHOLDER).size().x)
                .fold(0.0_f32, f32::max);
            let row_h = ui.spacing().interact_size.y;
            for (k, p) in c.props.iter().enumerate() {
                if fields.is_some_and(|visible| !visible.contains(&k)) {
                    continue;
                }
                if matches!(c.typ.as_str(), "ThPower" | "ThBoundary") && k == 3 {
                    let (mode, title) = if c.typ == "ThPower" {
                        ("発熱表", "時刻 [s], 発熱量 [W]")
                    } else {
                        (crate::thermal::BOUNDARY_TABLE, "時刻 [s], 温度 [°C]（線形補間）")
                    };
                    if crate::thermal::property(&c, 2) == mode {
                        ui.label(title);
                        let mut table = self.prop_buf[k].replace(';', "\n");
                        let response = ui.add(egui::TextEdit::multiline(&mut table).desired_rows(5).desired_width(f32::INFINITY).code_editor());
                        self.property_editing |= response.has_focus();
                        self.prop_buf[k] = table.replace('\n', ";").replace('\r', "");
                        if response.lost_focus() && self.prop_buf[k] != p.value {
                            commit = Some((k, self.prop_buf[k].clone()));
                        }
                    }
                    continue;
                }
                let pname = library::prop_name(&c, k);
                let desc = info.and_then(|i| i.props.get(k)).map(|p| p.description.clone()).unwrap_or_default();
                if k >= self.prop_buf.len() {
                    self.prop_buf.push(p.value.clone());
                }
                // "[a, b, c]" in the description -> choice list (like Qucs-S' property dialog)
                let options: Vec<String> = desc
                    .rsplit_once('[')
                    .and_then(|(_, r)| r.split_once(']'))
                    .map(|(o, _)| o.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                    .unwrap_or_default();
                let fill = if k % 2 == 1 { ui.visuals().faint_bg_color } else { egui::Color32::TRANSPARENT };
                egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(2, 2)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(egui::vec2(label_w, row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            // pad every name to the longest one so all value fields start at the same x
                            ui.set_min_width(label_w);
                            ui.label(&pname).on_hover_text(&desc);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut vis = p.visible();
                            if !(crate::thermal::is_element(&c.typ) && options.len() >= 2)
                                && ui.checkbox(&mut vis, "").on_hover_text("回路図に表示").changed() {
                                toggle = Some(k);
                            }
                            let w = ui.available_width().max(40.0);
                            if options.len() >= 2 && options.len() <= 30 {
                                let mut v = p.value.clone();
                                egui::ComboBox::from_id_salt(("prop", k)).width(w).selected_text(&v).show_ui(ui, |ui| {
                                    for o in &options {
                                        ui.selectable_value(&mut v, o.clone(), o);
                                    }
                                });
                                if v != p.value {
                                    self.prop_buf[k] = v.clone();
                                    commit = Some((k, v));
                                }
                            } else {
                                let r = ui.add_sized([w, row_h], egui::TextEdit::singleline(&mut self.prop_buf[k]));
                                self.property_editing |= r.has_focus();
                                if r.lost_focus() && self.prop_buf[k] != p.value {
                                    commit = Some((k, self.prop_buf[k].clone()));
                                }
                            }
                        });
                    });
                });
            }
            if fields.is_some() {
                let mut preview = c.clone();
                for (prop, value) in preview.props.iter_mut().zip(&self.prop_buf) {
                    prop.value.clone_from(value);
                }
                let derived = if c.typ == "ThCapacity" {
                    crate::thermal::capacity_from_dimensions(&preview).map(|v| (v, "熱容量", "J/K"))
                } else {
                    crate::thermal::resistance_from_dimensions(&preview).map(|v| (v, "熱抵抗", "K/W"))
                };
                if let Some((derived, label, unit)) = derived {
                    ui.separator();
                    match derived {
                        Ok(quantity) => {
                            let value = if (0.001..100_000.0).contains(&quantity) {
                                format!("{quantity:.6}")
                            } else {
                                format!("{quantity:.6e}")
                            };
                            ui.label(format!("計算した{label}  {value} {unit}"));
                        }
                        Err(error) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                    }
                }
            }
        });
        if let Some((k, v)) = commit {
            self.doc.checkpoint();
            let component = &mut self.doc.sch.components[ci];
            if let Some(info) = info.filter(|_| k >= component.props.len() || (k == 1 && crate::thermal::is_branch(&c.typ))) {
                library::fill_missing_properties(component, info);
            }
            component.props[k].value = v;
            self.changed();
            self.sel = vec![ci];
        }
        if let Some(k) = toggle {
            self.doc.checkpoint();
            let component = &mut self.doc.sch.components[ci];
            if let Some(info) = info.filter(|_| k >= component.props.len()) {
                library::fill_missing_properties(component, info);
            }
            let p = &mut component.props[k];
            p.display = if p.visible() { "0".into() } else { "1".into() };
            self.changed();
            self.sel = vec![ci];
        }
    }


    // ------------------------------------------------------------------ tools

    fn refresh_issues(&mut self) {
        self.issues = check::check(&self.doc.sch, &self.conn).issues;
        if self.issue_focus.is_some_and(|point| !self.issues.iter().any(|issue| issue.at == point)) {
            self.issue_focus = None;
            self.center_on_issue = false;
        }
    }

    fn issue_panel(&mut self, ctx: &egui::Context) {
        if !self.show_issue_panel {
            return;
        }
        let mut focus = None;
        let mut junction = None;
        egui::TopBottomPanel::bottom("issues").resizable(true).default_height(150.0).show(ctx, |ui| {
            let errors = self.issues.iter().filter(|issue| issue.severity == "error").count();
            let warnings = self.issues.len() - errors;
            ui.horizontal(|ui| {
                ui.heading("回路チェック");
                ui.label(format!("エラー {errors} / 警告 {warnings}"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("閉じる").clicked() {
                        self.show_issue_panel = false;
                    }
                });
            });
            if self.doc.sch.components.is_empty() && self.doc.sch.wires.is_empty() {
                ui.label("部品と配線を配置するとチェックできます。");
                return;
            }
            if self.issues.is_empty() {
                ui.label("チェック完了。問題は見つかりませんでした。");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (index, issue) in self.issues.iter().enumerate() {
                    let (severity, color) = if issue.severity == "error" { ("エラー", self.pal().error) } else { ("警告", self.pal().warn) };
                    ui.push_id(index, |ui| {
                        ui.separator();
                        ui.horizontal_wrapped(|ui| {
                            if ui.selectable_label(self.issue_focus == Some(issue.at), "場所を表示").clicked() {
                                focus = Some(issue.at);
                            }
                            ui.colored_label(color, severity);
                            ui.label(&issue.summary_ja);
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label(issue.guidance_ja());
                            if let Some(point) = issue.junction_point(&self.doc.sch) {
                                if ui.small_button("接続点を作成").on_hover_text("配線を分割し、この位置にあるピンや配線端を接続します").clicked() {
                                    junction = Some(point);
                                }
                            }
                        });
                    });
                }
            });
        });
        if let Some(point) = focus {
            self.issue_focus = Some(point);
            self.center_on_issue = true;
        }
        if let Some(point) = junction {
            self.doc.checkpoint();
            self.doc.sch.wires = crate::doc::split_wires_at(std::mem::take(&mut self.doc.sch.wires), point);
            self.sel_wires.clear();
            self.status = format!("接続点（{}, {}）を作成しました", point.0, point.1);
            self.changed();
        }
        if !self.show_issue_panel {
            self.issue_focus = None;
        }
    }

    // ------------------------------------------------------------------ canvas

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let canvas = resp.rect;
        self.canvas_rect = canvas;
        painter.rect_filled(canvas, 0.0, self.pal().canvas);
        self.rebuild_scene();
        if self.need_fit {
            self.fit(canvas);
            self.need_fit = false;
        }
        if self.center_on_issue {
            if let Some((x, y)) = self.issue_focus {
                self.zoom = self.zoom.max(1.0);
                self.offset = Vec2::new(x as f32, y as f32) - canvas.size() / 2.0 / self.zoom;
            }
            self.center_on_issue = false;
        }
        let ctx = ui.ctx().clone();
        let hover = resp.hover_pos();
        if let Some(h) = hover {
            self.cursor = self.snapped(canvas, h);
            // zoom
            let zd = ctx.input(|i| i.zoom_delta());
            let scroll = ctx.input(|i| i.smooth_scroll_delta);
            let factor = if (zd - 1.0).abs() > 1e-4 { zd } else if scroll.y.abs() > 0.0 { (scroll.y * 0.0025).exp() } else { 1.0 };
            if (factor - 1.0).abs() > 1e-4 {
                let before = self.to_sch_f(canvas, h);
                self.zoom = (self.zoom * factor).clamp(0.1, 12.0);
                let after = self.to_sch_f(canvas, h);
                self.offset += Vec2::new((before.0 - after.0) as f32, (before.1 - after.1) as f32);
            }
        }
        // pan with middle / right drag
        if resp.dragged_by(egui::PointerButton::Middle) || (resp.dragged_by(egui::PointerButton::Secondary) && self.tool == Tool::Select) {
            self.offset -= resp.drag_delta() / self.zoom;
        }

        if self.show_grid {
            self.paint_grid(&painter, canvas);
        }
        let shapes = std::mem::take(&mut self.scene.shapes);
        self.paint_shapes(&painter, canvas, &shapes, 1.0);
        self.scene.shapes = shapes;
        self.paint_thermal(&painter, canvas);
        if let Some((x, y)) = self.issue_focus {
            let point = self.to_screen(canvas, x, y);
            painter.circle_stroke(point, 13.0, Stroke::new(3.0_f32, self.pal().canvas));
            painter.circle_stroke(point, 13.0, Stroke::new(1.8_f32, self.pal().accent_hi));
        }

        let pointer = hover.map(|h| self.to_sch_f(canvas, h));
        match self.tool.clone() {
            Tool::Select => self.select_tool(&resp, canvas, &painter, pointer),
            Tool::Place(t) => {
                if let Some((px, py)) = pointer {
                    let (x, y) = (snap(px.round() as i32), snap(py.round() as i32));
                    if let Ok(mut c) = library::new_component(&t, "?", x, y) {
                        c.rotate = self.place_rot;
                        let pl = library::place(&c);
                        let mut ghost = Vec::new();
                        for p in &pl.prims {
                            crate::render::prim_shapes(p, &mut ghost, Some(theme::to_color(self.pal().accent)));
                        }
                        self.paint_shapes(&painter, canvas, &ghost, 0.5);
                    }
                    if resp.clicked() {
                        self.doc.checkpoint();
                        match self.doc.add_component(&t, None, x, y, Some(self.place_rot), None, &serde_json::Map::new()) {
                            Ok(n) => {
                                self.status = format!("{n} を配置");
                                self.changed();
                                self.sel = vec![self.doc.sch.components.len() - 1];
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
                if resp.secondary_clicked() {
                    self.tool = Tool::Select;
                }
            }
            Tool::Wire => {
                if let Some((px, py)) = pointer {
                    let cur = (snap(px.round() as i32), snap(py.round() as i32));
                    if let Some(&last) = self.wire_pts.last() {
                        let mid = (cur.0, last.1);
                        let s = Stroke::new(2.0_f32, self.pal().accent);
                        painter.line_segment([self.to_screen(canvas, last.0 as f64, last.1 as f64), self.to_screen(canvas, mid.0 as f64, mid.1 as f64)], s);
                        painter.line_segment([self.to_screen(canvas, mid.0 as f64, mid.1 as f64), self.to_screen(canvas, cur.0 as f64, cur.1 as f64)], s);
                    }
                    painter.circle_stroke(self.to_screen(canvas, cur.0 as f64, cur.1 as f64), 4.0, Stroke::new(1.0_f32, self.pal().accent));
                    if resp.double_clicked() || resp.secondary_clicked() {
                        self.finish_wire();
                    } else if resp.clicked() {
                        if let Some(&last) = self.wire_pts.last() {
                            if last.0 != cur.0 && last.1 != cur.1 {
                                self.wire_pts.push((cur.0, last.1));
                            }
                        }
                        if self.wire_pts.last() != Some(&cur) {
                            self.wire_pts.push(cur);
                        }
                    }
                }
            }
            Tool::AutoWire => {
                if let Some((px, py)) = pointer {
                    if let Some(pin) = self.pin_at(px, py) {
                        if let Some(p) = self.conn.find_pin(&pin) {
                            painter.circle_stroke(self.to_screen(canvas, p.x as f64, p.y as f64), 7.0, Stroke::new(2.0_f32, self.pal().accent_hi));
                            painter.text(self.to_screen(canvas, p.x as f64, p.y as f64) + Vec2::new(8.0, -18.0), Align2::LEFT_TOP, &pin, FontId::proportional(13.0), self.pal().accent_hi);
                        }
                        if resp.clicked() {
                            match self.autowire_first.take() {
                                None => self.autowire_first = Some(pin),
                                Some(a) => {
                                    self.doc.checkpoint();
                                    match self.doc.connect(&[a.clone(), pin.clone()], None) {
                                        Ok(r) => self.status = format!("{a} — {pin}: {}", r.get("routes").map(|x| x.to_string()).unwrap_or_default()),
                                        Err(e) => self.status = e,
                                    }
                                    self.changed();
                                }
                            }
                        }
                    }
                    if let Some(a) = &self.autowire_first {
                        if let Some(p) = self.conn.find_pin(a) {
                            painter.line_segment([self.to_screen(canvas, p.x as f64, p.y as f64), hover.unwrap()], Stroke::new(1.0_f32, self.pal().accent_hi));
                        }
                    }
                }
                if resp.secondary_clicked() {
                    self.autowire_first = None;
                }
            }
            Tool::Paint(kind) => {
                if let Some((px, py)) = pointer {
                    let cur = (snap(px.round() as i32), snap(py.round() as i32));
                    if kind == "text" {
                        if resp.clicked() {
                            self.text_prompt = Some((cur, String::new(), None));
                        }
                    } else {
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            let press = resp.ctx.input(|i| i.pointer.press_origin()).unwrap_or(hover.unwrap());
                            self.paint_start = Some(self.snapped(canvas, press));
                        }
                        if let Some(st) = self.paint_start {
                            let a = self.to_screen(canvas, st.0 as f64, st.1 as f64);
                            let b = self.to_screen(canvas, cur.0 as f64, cur.1 as f64);
                            let s = Stroke::new(1.5_f32, self.pal().accent);
                            match kind.as_str() {
                                "rect" => {
                                    painter.rect_stroke(Rect::from_two_pos(a, b), 0.0, s, egui::StrokeKind::Inside);
                                }
                                "ellipse" => {
                                    let r = Rect::from_two_pos(a, b);
                                    painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, s));
                                }
                                _ => {
                                    painter.line_segment([a, b], s);
                                }
                            }
                            if resp.drag_stopped() {
                                self.paint_start = None;
                                let (x, y, w, h) = match kind.as_str() {
                                    "rect" | "ellipse" => (st.0.min(cur.0), st.1.min(cur.1), (cur.0 - st.0).abs(), (cur.1 - st.1).abs()),
                                    _ => (st.0, st.1, cur.0 - st.0, cur.1 - st.1),
                                };
                                if w != 0 || h != 0 {
                                    if let Ok(line) = crate::edit::make_painting(&kind, x, y, w, h, "#000080", 2, "", 12, false) {
                                        self.doc.checkpoint();
                                        self.doc.sch.paintings.push(format!("  {line}"));
                                        self.changed();
                                    }
                                }
                            }
                        }
                    }
                }
                if resp.secondary_clicked() {
                    self.tool = Tool::Select;
                }
            }
            Tool::Label => {
                if let Some((px, py)) = pointer {
                    if resp.clicked() {
                        let target = if let Some(pin) = self.pin_at(px, py) {
                            Some(pin)
                        } else if let Some(wi) = self.wire_at(px, py) {
                            let w = &self.doc.sch.wires[wi];
                            Some(format!("[{},{}]", w.x1, w.y1))
                        } else {
                            None
                        };
                        if let Some(t) = target {
                            self.label_prompt = Some((t, String::new()));
                        }
                    }
                }
            }
        }

        // status text overlay (coordinates)
        painter.text(canvas.right_bottom() - Vec2::new(8.0, 6.0), Align2::RIGHT_BOTTOM, format!("x={} y={}  zoom {:.0}%", self.cursor.0, self.cursor.1, self.zoom * 100.0 / 1.5), FontId::proportional(12.0), self.pal().muted);
        self.write_state(canvas);
    }

    fn finish_wire(&mut self) {
        if self.wire_pts.len() >= 2 {
            self.doc.checkpoint();
            let pts = std::mem::take(&mut self.wire_pts);
            let n = self.doc.add_wire_path(&pts);
            self.status = format!("配線 {n} 本を追加");
            self.changed();
        }
        self.wire_pts.clear();
    }

    fn select_tool(&mut self, resp: &egui::Response, canvas: Rect, painter: &egui::Painter, pointer: Option<(f64, f64)>) {
        let primary_down = resp.ctx.input(|i| i.pointer.primary_down());
        // a wire under the pointer (and no part) can be dragged: show the direction it moves
        let wire_dir = match &self.grab {
            Some(Grab::Wire { wire, orig, .. }) => orig.get(*wire).cloned(),
            Some(Grab::Group { .. }) => Some(crate::schematic::Wire::new(0, 0, 0, 0)),
            Some(_) => None,
            None if self.drag.is_none() && self.box_start.is_none() => pointer
                .filter(|&(px, py)| self.comp_at(px, py).is_none() && crate::edit::wire_label_at(&self.doc, px, py).is_none())
                // (matches the drag-start order: wire label, wire, component text, part)
                .and_then(|(px, py)| self.wire_at(px, py))
                .map(|wi| {
                    if self.sel_wires.contains(&wi) && self.sel.len() + self.sel_wires.len() + self.sel_paint.len() > 1 {
                        // part of a selection: the whole block moves freely
                        crate::schematic::Wire::new(0, 0, 0, 0)
                    } else {
                        self.doc.sch.wires[wi].clone()
                    }
                }),
            None => None,
        };
        if let Some(w) = wire_dir {
            resp.ctx.set_cursor_icon(if w.is_point() || (!w.is_horizontal() && !w.is_vertical()) {
                egui::CursorIcon::Move
            } else if w.is_horizontal() {
                egui::CursorIcon::ResizeVertical
            } else {
                egui::CursorIcon::ResizeHorizontal
            });
        }
        if resp.double_clicked() {
            if let Some((px, py)) = pointer {
                if let Some(pi) = crate::edit::painting_at(&self.doc, px, py) {
                    let line = &self.doc.sch.paintings[pi];
                    if line.trim_start().starts_with("<Text") {
                        let t = crate::schematic::painting_tokens(line);
                        let cur = t.get(6).map(|s| crate::schematic::unquote(s).replace("\\n", "\n")).unwrap_or_default();
                        self.text_prompt = Some(((0, 0), cur, Some(pi)));
                    }
                }
            }
        }
        // selected paintings: dashed outline
        for &k in &self.sel_paint {
            if let Some(b) = self.doc.sch.paintings.get(k).and_then(|p| crate::edit::painting_bbox(p)) {
                let r = Rect::from_two_pos(self.to_screen(canvas, b[0] - 3.0, b[1] - 3.0), self.to_screen(canvas, b[2] + 3.0, b[3] + 3.0));
                painter.rect_stroke(r, 2.0, Stroke::new(1.5_f32, self.pal().accent_hi), egui::StrokeKind::Outside);
            }
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(press) = resp.ctx.input(|i| i.pointer.press_origin()) {
                let (px, py) = self.to_sch_f(canvas, press);
                if let Some(wi) = crate::edit::wire_label_at(&self.doc, px, py) {
                    let w = &self.doc.sch.wires[wi];
                    self.grab = Some(Grab::WireLabel { wire: wi, orig: (w.lx, w.ly), start: (px, py) });
                } else if let Some(wi) = self.wire_at(px, py).filter(|_| self.comp_at(px, py).is_none()) {
                    // a wire wins over a component text block lying across it
                    if self.sel_wires.contains(&wi) && self.sel.len() + self.sel_wires.len() + self.sel_paint.len() > 1 {
                        self.start_group(px, py);
                        return;
                    }
                    self.sel.clear();
                    self.sel_paint.clear();
                    self.sel_wires = vec![wi];
                    let pins = self.conn.pins.iter().map(|p| (p.x, p.y)).collect();
                    self.grab = Some(Grab::Wire { wire: wi, orig: self.doc.sch.wires.clone(), pins, start: (px.round() as i32, py.round() as i32), moved: (0, 0) });
                } else if let Some(ci) = crate::edit::comp_label_at(&self.doc, px, py).filter(|ci| self.sel.contains(ci) || self.comp_at(px, py).is_none()) {
                    let c = &self.doc.sch.components[ci];
                    self.grab = Some(Grab::CompLabel { comp: ci, orig: (c.tx, c.ty), start: (px, py) });
                } else if let Some(ci) = self.comp_at(px, py) {
                    if !self.sel.contains(&ci) {
                        self.sel = vec![ci];
                        self.sel_wires.clear();
                        self.sel_paint.clear();
                    } else if !self.sel_wires.is_empty() || !self.sel_paint.is_empty() {
                        self.start_group(px, py);
                        return;
                    }
                    let orig = self.sel.iter().map(|&i| (i, self.doc.sch.components[i].x, self.doc.sch.components[i].y)).collect();
                    self.drag = Some(Drag { start: (px.round() as i32, py.round() as i32), orig, moved: false });
                } else if let Some(pi) = crate::edit::painting_at(&self.doc, px, py) {
                    if !self.sel_paint.contains(&pi) {
                        self.sel_paint = vec![pi];
                    } else if !self.sel.is_empty() || !self.sel_wires.is_empty() {
                        self.start_group(px, py);
                        return;
                    }
                    let orig = self.sel_paint.iter().map(|&k| (k, self.doc.sch.paintings[k].clone())).collect();
                    self.grab = Some(Grab::Paintings { orig, start: (px.round() as i32, py.round() as i32), moved: (0, 0) });
                } else {
                    self.box_start = Some(press);
                }
            }
        }
        if let (Some(d), Some((px, py))) = (&mut self.drag, pointer) {
            if primary_down {
                let dx = snap(px.round() as i32 - d.start.0);
                let dy = snap(py.round() as i32 - d.start.1);
                for &(i, ox, oy) in &d.orig {
                    let c = &mut self.doc.sch.components[i];
                    if (c.x, c.y) != (ox + dx, oy + dy) {
                        c.x = ox + dx;
                        c.y = oy + dy;
                        d.moved = true;
                    }
                }
                if d.moved {
                    self.conn = self.doc.conn();
                    self.gen += 1;
                }
            }
        }
        if let (Some(g), Some((px, py))) = (&mut self.grab, pointer) {
            if primary_down {
                match g {
                    Grab::WireLabel { wire, orig, start } => {
                        let w = &mut self.doc.sch.wires[*wire];
                        w.lx = orig.0 + (px - start.0).round() as i32;
                        w.ly = orig.1 + (py - start.1).round() as i32;
                    }
                    Grab::CompLabel { comp, orig, start } => {
                        let c = &mut self.doc.sch.components[*comp];
                        c.tx = orig.0 + (px - start.0).round() as i32;
                        c.ty = orig.1 + (py - start.1).round() as i32;
                    }
                    Grab::Paintings { orig, start, moved } => {
                        let d = (snap(px.round() as i32 - start.0), snap(py.round() as i32 - start.1));
                        if d != *moved {
                            *moved = d;
                            for (k, line) in orig.iter() {
                                self.doc.sch.paintings[*k] = crate::edit::translate_painting(line, d.0, d.1);
                            }
                        }
                    }
                    Grab::Wire { wire, orig, pins, start, moved } => {
                        let d = (snap(px.round() as i32 - start.0), snap(py.round() as i32 - start.1));
                        if d != *moved {
                            *moved = d;
                            self.doc.sch.wires = crate::edit::drag_wire(orig, pins, *wire, d.0, d.1);
                        }
                    }
                    Grab::Group { orig, pins, comps, wires, paints, start, moved } => {
                        let d = (snap(px.round() as i32 - start.0), snap(py.round() as i32 - start.1));
                        if d != *moved {
                            *moved = d;
                            let mut s = (**orig).clone();
                            crate::edit::move_group(&mut s, pins, comps, wires, paints, d.0, d.1);
                            self.doc.sch = s;
                        }
                    }
                }
                self.conn = self.doc.conn();
                self.gen += 1;
            }
        }
        if let Some(b) = self.box_start {
            if let Some(h) = resp.hover_pos().or(resp.interact_pointer_pos()) {
                let r = Rect::from_two_pos(b, h);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, self.pal().accent), egui::StrokeKind::Inside);
                painter.rect_filled(r, 0.0, self.pal().accent.gamma_multiply(0.12));
            }
        }
        if resp.drag_stopped() {
            if let Some(g) = self.grab.take() {
                // put the original back, checkpoint, re-apply: one undo step
                let snapshot = self.doc.sch.clone();
                match &g {
                    Grab::WireLabel { wire, orig, .. } => {
                        self.doc.sch.wires[*wire].lx = orig.0;
                        self.doc.sch.wires[*wire].ly = orig.1;
                    }
                    Grab::CompLabel { comp, orig, .. } => {
                        self.doc.sch.components[*comp].tx = orig.0;
                        self.doc.sch.components[*comp].ty = orig.1;
                    }
                    Grab::Paintings { orig, .. } => {
                        for (k, line) in orig {
                            self.doc.sch.paintings[*k] = line.clone();
                        }
                    }
                    Grab::Wire { orig, .. } => self.doc.sch.wires = orig.clone(),
                    Grab::Group { orig, .. } => self.doc.sch = (**orig).clone(),
                }
                self.doc.checkpoint();
                self.doc.sch = snapshot;
                if let Grab::Wire { wire, pins, .. } = &g {
                    // merge the stretched / split wires and keep the dragged one selected
                    let moved = self.doc.sch.wires[*wire].clone();
                    self.doc.sch.wires = crate::doc::normalize_wires(std::mem::take(&mut self.doc.sch.wires), pins);
                    let on = |w: &crate::schematic::Wire, p: (i32, i32)| {
                        (w.x1.min(w.x2)..=w.x1.max(w.x2)).contains(&p.0) && (w.y1.min(w.y2)..=w.y1.max(w.y2)).contains(&p.1)
                    };
                    self.sel_wires = self.doc.sch.wires.iter().position(|w| on(w, moved.p1()) && on(w, moved.p2())).into_iter().collect();
                }
                if let Grab::Group { wires, moved: d, .. } = &g {
                    // merge stretched wires; keep the moved wires selected
                    let moved: Vec<crate::schematic::Wire> = wires.iter().filter_map(|&i| self.doc.sch.wires.get(i).cloned()).collect();
                    let pins = self.doc.conn().pins.iter().map(|p| (p.x, p.y)).collect();
                    self.doc.sch.wires = crate::doc::normalize_wires(std::mem::take(&mut self.doc.sch.wires), &pins);
                    self.sel_wires = wire_cover(&self.doc.sch.wires, &moved);
                    self.status = format!("{} 個を移動 ({}, {})", self.sel.len() + moved.len() + self.sel_paint.len(), d.0, d.1);
                }
                self.changed();
            }
            if let Some(d) = self.drag.take() {
                if d.moved {
                    // restore, then apply as one undoable move (+ re-route)
                    let finals: Vec<(String, i32, i32)> = d.orig.iter().map(|&(i, _, _)| {
                        let c = &self.doc.sch.components[i];
                        (c.name.clone(), c.x, c.y)
                    }).collect();
                    for &(i, ox, oy) in &d.orig {
                        self.doc.sch.components[i].x = ox;
                        self.doc.sch.components[i].y = oy;
                    }
                    self.doc.checkpoint();
                    if let Err(e) = self.doc.move_components(&finals, self.auto_reroute) {
                        self.status = e;
                    }
                    self.changed();
                }
            }
            if let Some(b) = self.box_start.take() {
                if let Some(h) = resp.interact_pointer_pos().or(resp.hover_pos()) {
                    let (x0, y0) = self.to_sch_f(canvas, b);
                    let (x1, y1) = self.to_sch_f(canvas, h);
                    let r = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
                    self.sel = self.conn.placed.iter().enumerate().filter(|(_, p)| p.bbox[0] < r[2] && p.bbox[2] > r[0] && p.bbox[1] < r[3] && p.bbox[3] > r[1]).map(|(i, _)| i).collect();
                    self.sel_paint = self.doc.sch.paintings.iter().enumerate().filter(|(_, p)| {
                        crate::edit::painting_bbox(p).map(|b| b[0] >= r[0] && b[2] <= r[2] && b[1] >= r[1] && b[3] <= r[3]).unwrap_or(false)
                    }).map(|(i, _)| i).collect();
                    self.sel_wires = self.doc.sch.wires.iter().enumerate().filter(|(_, w)| {
                        let (a, bb) = ((w.x1.min(w.x2)) as f64, (w.x1.max(w.x2)) as f64);
                        let (c, dd) = ((w.y1.min(w.y2)) as f64, (w.y1.max(w.y2)) as f64);
                        a >= r[0] && bb <= r[2] && c >= r[1] && dd <= r[3]
                    }).map(|(i, _)| i).collect();
                }
            }
        }
        if resp.clicked() {
            if let Some((px, py)) = pointer {
                let shift = resp.ctx.input(|i| i.modifiers.shift);
                if let Some(ci) = self.comp_at(px, py) {
                    if shift {
                        if let Some(k) = self.sel.iter().position(|&x| x == ci) {
                            self.sel.remove(k);
                        } else {
                            self.sel.push(ci);
                        }
                    } else {
                        self.sel = vec![ci];
                        self.sel_wires.clear();
                    }
                    self.sel_paint.clear();
                } else if let Some(wi) = self.wire_at(px, py) {
                    if !shift {
                        self.sel.clear();
                        self.sel_wires.clear();
                        self.sel_paint.clear();
                    }
                    self.sel_wires.push(wi);
                } else if let Some(pi) = crate::edit::painting_at(&self.doc, px, py) {
                    if !shift {
                        self.sel.clear();
                        self.sel_wires.clear();
                        self.sel_paint.clear();
                    }
                    self.sel_paint.push(pi);
                } else {
                    self.sel.clear();
                    self.sel_wires.clear();
                    self.sel_paint.clear();
                }
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context, canvas: Rect) {
        if self.external_review.is_some() {
            return;
        }
        let wants_keyboard = ctx.wants_keyboard_input();
        if (!wants_keyboard || self.property_editing) && ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.open_thermal();
            return;
        }
        if wants_keyboard {
            return;
        }
        let (ctrl, keys) = ctx.input(|i| (i.modifiers.command, i.keys_down.clone()));
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        let _ = keys;
        if ctrl && pressed(egui::Key::Z) {
            if self.doc.undo() {
                self.changed();
            }
            return;
        }
        if ctrl && pressed(egui::Key::Y) {
            if self.doc.redo() {
                self.changed();
            }
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::Copy => self.copy_selection(ctx, false),
                egui::Event::Cut => self.copy_selection(ctx, true),
                egui::Event::Paste(text) => self.paste(&text),
                _ => {}
            }
        }
        if ctrl && pressed(egui::Key::T) {
            self.new_tab(Doc::new(self.blank_schematic(), None));
            return;
        }
        if ctrl && pressed(egui::Key::W) {
            self.close_tab(self.active);
            return;
        }
        if ctrl && pressed(egui::Key::Tab) {
            let n = self.tabs.len();
            let shift = ctx.input(|i| i.modifiers.shift);
            self.switch_tab(if shift { (self.active + n - 1) % n } else { (self.active + 1) % n });
            return;
        }
        if ctrl && pressed(egui::Key::O) {
            self.open_dialog();
            return;
        }
        if ctrl && pressed(egui::Key::D) {
            self.duplicate_selection();
            return;
        }
        if ctrl && pressed(egui::Key::A) {
            self.sel = (0..self.doc.sch.components.len()).collect();
            self.sel_wires = (0..self.doc.sch.wires.len()).collect();
            self.sel_paint = (0..self.doc.sch.paintings.len()).collect();
            return;
        }
        if ctrl && pressed(egui::Key::S) {
            if ctx.input(|i| i.modifiers.shift) {
                self.save_as();
            } else {
                self.save_current();
            }
            return;
        }
        if ctrl && pressed(egui::Key::F) {
            self.find_open = true;
            self.find_focus = true;
            return;
        }
        if ctrl && pressed(egui::Key::P) {
            self.print();
            return;
        }
        if ctrl && (pressed(egui::Key::Plus) || pressed(egui::Key::Equals)) {
            self.zoom_by(1.25);
            return;
        }
        if ctrl && pressed(egui::Key::Minus) {
            self.zoom_by(0.8);
            return;
        }
        if ctrl && pressed(egui::Key::Num0) {
            self.fit(canvas);
            return;
        }
        if pressed(egui::Key::F1) {
            self.about_open = true;
            return;
        }
        if ctrl {
            return;
        }
        if pressed(egui::Key::Escape) {
            if self.tool == Tool::Wire {
                self.finish_wire();
            }
            self.tool = Tool::Select;
            self.autowire_first = None;
        }
        if pressed(egui::Key::R) {
            if let Tool::Place(_) = self.tool {
                self.place_rot = (self.place_rot + 1) % 4;
            } else {
                self.rotate_selected();
            }
        }
        if pressed(egui::Key::M) {
            self.mirror_selected();
        }
        if pressed(egui::Key::Y) {
            self.mirror_y_selected();
        }
        if pressed(egui::Key::T) {
            self.tool = Tool::Paint("text".into());
        }
        if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
            self.delete_selected();
        }
        if pressed(egui::Key::W) {
            self.tool = Tool::Wire;
        }
        if pressed(egui::Key::A) {
            self.tool = Tool::AutoWire;
        }
        if pressed(egui::Key::L) {
            self.tool = Tool::Label;
        }
        if pressed(egui::Key::F) {
            self.fit(canvas);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && (0..self.tabs.len()).any(|i| self.tab_unsaved(i)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Quit);
        }
        self.poll_disk();
        self.poll_open_requests();
        self.heartbeat();
        let title = format!(
            "Qucs-T — {}{}",
            self.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "無題".into()),
            if self.tab_unsaved(self.active) { " *" } else { "" }
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let th = ctx.theme();
        if th != self.tex_theme {
            self.tex_theme = th;
            self.gen += 1;
        }

        let screen = ctx.content_rect();
        let pal = self.pal();
        let bar = egui::Frame::new().fill(pal.bar).inner_margin(egui::Margin::symmetric(10, 6)).stroke(egui::Stroke::new(1.0_f32, pal.border));
        egui::TopBottomPanel::top("toolbar").frame(bar).show(ctx, |ui| self.toolbar(ui));
        egui::TopBottomPanel::top("tabs").frame(egui::Frame::new().fill(pal.bar).inner_margin(egui::Margin::symmetric(10, 3))).show(ctx, |ui| self.tab_bar(ui));
        egui::TopBottomPanel::bottom("status").frame(bar.inner_margin(egui::Margin::symmetric(10, 3))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("ⓘ").on_hover_text("バージョン情報 (F1)").clicked() {
                        self.about_open = true;
                    }
                    self.theme_menu(ui);
                    let check_label = if self.show_issues || self.show_issue_panel {
                        let errors = self.issues.iter().filter(|issue| issue.severity == "error").count();
                        format!("エラー {errors} / 警告 {}", self.issues.len() - errors)
                    } else {
                        "回路チェック".into()
                    };
                    if ui.small_button(check_label).clicked() {
                        self.show_issue_panel = !self.show_issue_panel;
                        if self.show_issue_panel {
                            self.refresh_issues();
                        } else {
                            self.issue_focus = None;
                        }
                    }
                    ui.label(format!("部品 {}  配線 {}  ネット {}", self.doc.sch.components.len(), self.doc.sch.wires.len(), self.conn.nets.len()));
                });
            });
        });
        if self.external_change {
            egui::TopBottomPanel::top("ext").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(self.pal().warn, "外部との変更が競合しています。元のファイルへの保存を保留しています。");
                    if ui.button("変更を確認…").clicked() {
                        self.external_review = self.doc.path.as_deref().map(Doc::open);
                    }
                });
            });
        }
        self.issue_panel(ctx);
        let thermal_panel_visible = self.thermal.open;
        if self.thermal_panel_visible != thermal_panel_visible {
            self.thermal_panel_visible = thermal_panel_visible;
            self.need_fit = true;
        }
        let palette_max_width = (screen.width() * 0.42).max(120.0);
        let lw = egui::SidePanel::left("palette").default_width(self.settings.left_panel).width_range(120.0..=palette_max_width)
            .show(ctx, |ui| self.thermal_palette(ui)).response.rect.width();
        if !self.thermal.open {
            self.settings.right_panel = egui::SidePanel::right("props").default_width(self.settings.right_panel)
                .show(ctx, |ui| self.properties(ui)).response.rect.width();
        }
        self.thermal_panel(ctx);
        self.settings.left_panel = lw;
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            let canvas = ui.max_rect();
            self.keys(ctx, canvas);
            self.canvas(ui);
        });
        self.cylinder_window(ctx);
        self.temperature_map_window(ctx);
        self.find_window(ctx);
        self.about_window(ctx);
        self.confirm_dialog(ctx);
        self.external_change_dialog(ctx);
        if let Some((pos, mut text, editing)) = self.text_prompt.take() {
            let mut done = false;
            let mut cancel = false;
            egui::Window::new("テキスト").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3));
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        done = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
            if done && !text.trim().is_empty() {
                self.doc.checkpoint();
                match editing {
                    Some(k) => self.doc.sch.paintings[k] = crate::edit::set_painting_text(&self.doc.sch.paintings[k], &text),
                    None => {
                        if let Ok(l) = crate::edit::make_painting("text", pos.0, pos.1, 0, 0, "#000000", 1, &text, 12, false) {
                            self.doc.sch.paintings.push(format!("  {l}"));
                        }
                    }
                }
                self.changed();
            } else if !done && !cancel {
                self.text_prompt = Some((pos, text, editing));
            }
        }
        if let Some((t, mut text)) = self.label_prompt.take() {
            let mut done = false;
            let mut cancel = false;
            egui::Window::new("ネット名").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(format!("対象: {t}"));
                let r = ui.text_edit_singleline(&mut text);
                r.request_focus();
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        done = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
            if done && !text.trim().is_empty() {
                self.doc.checkpoint();
                match self.doc.add_net_label(&t, text.trim()) {
                    Ok(_) => self.status = format!("ネット名 {}", text.trim()),
                    Err(e) => self.status = e,
                }
                self.changed();
            } else if !done && !cancel {
                self.label_prompt = Some((t, text));
            }
        }
        self.drive_shot(ctx);
        // keep polling the file even when idle
        ctx.request_repaint_after(std::time::Duration::from_millis(350));
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.store_settings();
        // no longer accepting files: let the next launch open its own window
        let path = crate::mcp::gui_heartbeat_path();
        if std::fs::read_to_string(&path).ok().as_deref() == Some(std::process::id().to_string().as_str()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Indices of the wires in `wires` that lie on one of the `moved` wires (after merging).
fn wire_cover(wires: &[crate::schematic::Wire], moved: &[crate::schematic::Wire]) -> Vec<usize> {
    let inside = |w: &crate::schematic::Wire, p: (i32, i32)| (w.x1.min(w.x2)..=w.x1.max(w.x2)).contains(&p.0) && (w.y1.min(w.y2)..=w.y1.max(w.y2)).contains(&p.1);
    wires.iter().enumerate().filter(|(_, w)| moved.iter().any(|m| inside(m, w.p1()) && inside(m, w.p2()) || inside(w, m.p1()) && inside(w, m.p2()))).map(|(i, _)| i).collect()
}
