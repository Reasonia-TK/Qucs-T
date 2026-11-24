# Qucs-T

Qucs-T is a Windows desktop simulator for lumped thermal networks, based on the
Qucs-S-Rust schematic editor. It has a native Rust solver for steady-state and
transient analysis.

```powershell
cargo build --release
.\target\release\qucs-t.exe gui
```

Click **サンプル** to open the heat sink example, then **熱解析…** (F5) to run
**定常解析** (steady state) or **過渡解析** (transient). Results include node
temperatures, branch heat flows, and CSV export. Thermal schematics and their
analysis settings are stored in `.qucst` files.

Conduction, contact, convection, and radiation share a two-terminal symbol.
Their display labels and calculation models are edited independently. The solver
uses temperature differences for resistance and convection, and the fourth
power of absolute temperature for radiation.

See the [Japanese README](README.md) for editing and analysis details.
Qucs-T is licensed under GPL-2.0-or-later.
