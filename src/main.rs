//! Qucs-T thermal network simulator and schematic editor.

mod check;
mod doc;
mod edit;
mod gui;
mod icon_pixels;
mod library;
mod mcp;
mod netlist;
mod pdf;
mod plot;
mod render;
mod router;
mod schematic;
mod settings;
mod thermal;
mod thermal_grid;
mod thermal_measurement;
mod theme;

use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "Qucs-T {}\n\
         Usage:\n  \
         qucs-t [gui] [file.qucst]                Open the thermal editor\n  \
         qucs-t steady <file.qucst> [out.csv]    Steady-state analysis\n  \
         qucs-t transient <file.qucst> [out.csv] Transient analysis\n  \
         qucs-t check <file.qucst>              Check the layout\n  \
         qucs-t render <file.qucst> <out>       Export a PNG or SVG schematic\n  \
         qucs-t reroute <file.qucst> [out]      Reroute the schematic\n  \
         qucs-t mcp                             Start the stdio MCP server\n  \
         qucs-t --version",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("gui");
    match cmd {
        "steady" | "transient" => {
            if !(2..=3).contains(&args.len()) {
                usage();
            }
            let path = PathBuf::from(&args[1]);
            let doc = doc::Doc::open(&path).unwrap_or_else(|e| fail(&e));
            let network = thermal::Network::build(&doc.sch, &doc.conn()).unwrap_or_else(|e| fail(&e));
            let result = if cmd == "steady" {
                network.steady()
            } else {
                let settings = thermal::Settings::read(&doc.sch).unwrap_or_else(|e| fail(&e));
                network.transient(&settings)
            }.unwrap_or_else(|e| fail(&e));
            if let Some(csv) = args.get(2) {
                std::fs::write(csv, result.csv()).unwrap_or_else(|e| fail(&e.to_string()));
            } else {
                println!("{}", serde_json::to_string(&result).unwrap_or_else(|e| fail(&e.to_string())));
            }
        }
        "mcp" => mcp::serve(),
        "render" => {
            if args.len() < 3 {
                usage();
            }
            let d = doc::Doc::open(&PathBuf::from(&args[1])).unwrap_or_else(|e| fail(&e));
            let conn = d.conn();
            let overlay = args.iter().any(|a| a == "--overlay");
            let scale = args.iter().position(|a| a == "--scale").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok());
            let rep = check::check(&d.sch, &conn);
            let ov = render::Overlay {
                color_nets: overlay,
                net_tags: overlay,
                pin_names: args.iter().any(|a| a == "--pins"),
                markers: if overlay { rep.issues.iter().filter(|i| i.severity == "error" || i.kind == "unconnected_pin").map(|i| (i.at.0, i.at.1, i.message.clone())).collect() } else { vec![] },
                ..Default::default()
            };
            let sc = render::build_scene(&d.sch, &conn, &ov);
            let view = render::fit_view(sc.bounds, 30.0, scale, 1600.0);
            let (svg, _) = render::scene_svg(&sc, &view, &render::Decor { grid: overlay, rulers: overlay, title: None });
            let out = &args[2];
            if out.ends_with(".svg") {
                std::fs::write(out, svg).unwrap_or_else(|e| fail(&e.to_string()));
            } else {
                let png = render::svg_to_png(&svg).unwrap_or_else(|e| fail(&e));
                std::fs::write(out, png).unwrap_or_else(|e| fail(&e.to_string()));
            }
            println!("{}", rep.metrics);
        }
        "check" => {
            if args.len() < 2 {
                usage();
            }
            let d = doc::Doc::open(&PathBuf::from(&args[1])).unwrap_or_else(|e| fail(&e));
            let rep = check::check(&d.sch, &d.conn());
            let v = serde_json::json!({"metrics": rep.metrics, "issues": rep.issues.iter().enumerate().map(|(k, i)| i.json(k + 1)).collect::<Vec<_>>()});
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        "reroute" => {
            if args.len() < 2 {
                usage();
            }
            let mut d = doc::Doc::open(&PathBuf::from(&args[1])).unwrap_or_else(|e| fail(&e));
            let r = d.reroute(&[]).unwrap_or_else(|e| fail(&e));
            let out = PathBuf::from(args.get(2).unwrap_or(&args[1]));
            d.sch.save(&out).unwrap_or_else(|e| fail(&e));
            println!("{r}");
        }
        "-V" | "--version" | "version" => println!("Qucs-T {}", env!("CARGO_PKG_VERSION")),
        "-h" | "--help" | "help" => usage(),
        "gui" => gui::run_thermal(args.get(1).map(PathBuf::from)),
        "thermal-gui" => gui::run_thermal(args.get(1).map(PathBuf::from)),
        other if other.to_ascii_lowercase().ends_with(".qucst") => gui::run_thermal(Some(PathBuf::from(other))),
        _ => usage(),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use crate::doc::Doc;
    use crate::schematic::Schematic;
    use std::path::Path;

    #[test]
    fn roundtrip_example() {
        let sch = Doc::open(Path::new("examples/heatsink.qucst")).unwrap().sch;
        assert_eq!(Schematic::parse(&sch.serialize()).unwrap(), sch);
    }

    #[test]
    fn pins_follow_qucs_rotation() {
        // vertical resistor (rotate=1): pin 1 at the bottom, pin 2 at the top
        let c = crate::library::new_component("ThResistance", "R1", 100, 100).unwrap();
        let mut c1 = c.clone();
        c1.rotate = 1;
        let p = crate::library::place(&c1);
        assert_eq!((p.pins[0].x, p.pins[0].y), (100, 130));
        assert_eq!((p.pins[1].x, p.pins[1].y), (100, 70));
        assert_eq!(p.pins[0].dir, (0, 1));
        let power = crate::library::new_component("ThPower", "P1", 0, 0).unwrap();
        let p = crate::library::place(&power);
        assert_eq!((p.pins[0].x, p.pins[0].y), (0, -30));
    }

    #[test]
    fn example_connectivity() {
        let d = Doc::open(Path::new("examples/heatsink.qucst")).unwrap();
        let conn = d.conn();
        let net = |pin: &str| conn.nets[conn.find_pin(pin).unwrap().net].name.as_str();
        assert_eq!(net("P1.1"), "発熱部品");
        assert_eq!(net("Rc1.1"), net("P1.1"));
        assert_eq!(net("Rc1.2"), "ヒートシンク");
        assert_eq!(net("T1.1"), "空気");
        assert!(crate::check::check(&d.sch, &conn).issues.is_empty());
    }

    #[test]
    fn router_connects_cleanly() {
        let mut d = Doc::new(crate::thermal::empty(), None);
        let none = serde_json::Map::new();
        d.add_component("ThPower", Some("P1"), 0, 100, None, None, &none).unwrap();
        d.add_component("ThResistance", Some("R1"), 150, 40, None, None, &none).unwrap();
        d.add_component("ThResistance", Some("R2"), 260, 120, Some(1), None, &none).unwrap();
        d.add_component("ThBoundary", Some("T1"), 0, 220, None, None, &none).unwrap();
        d.connect(&["P1.1".into(), "R1.1".into()], Some("in")).unwrap();
        d.connect(&["R1.2".into(), "R2.2".into()], Some("out")).unwrap();
        d.connect(&["R2.1".into(), "T1.1".into()], Some("ambient")).unwrap();
        let conn = d.conn();
        let net = |pin: &str| conn.nets[conn.find_pin(pin).unwrap().net].name.clone();
        assert_eq!(net("R1.1"), "in");
        assert_eq!(net("R2.2"), "out");
        assert_eq!(net("R2.1"), "ambient");
        let rep = crate::check::check(&d.sch, &conn);
        assert_eq!(rep.metrics["errors"], 0, "{:?}", rep.issues);
        assert_eq!(rep.metrics["crossings"], 0);
        assert!(d.sch.wires.iter().all(|w| w.is_horizontal() || w.is_vertical()));
    }
}

#[cfg(test)]
mod core_tests;
