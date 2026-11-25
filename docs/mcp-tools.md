# MCP tool reference

Generated from `qucs-t mcp` (0.1.0) by `tools/gen_mcp_docs.py` — do not edit by hand.

Coordinates are Qucs units (x right, **y down**, 10-unit grid). Terminals: `"Rth1.1"`, points `"[x,y]"`, nets `"net:name"`.

## Recommended workflow

Qucs-T thermal network simulator and schematic editor. Coordinates are Qucs units: x to the right, y down, 10-unit grid. Pins are addressed as "Rth1.1", points as "[x,y]", and nets as "net:name". Start with new_thermal_schematic, open_schematic (.qucst), or generate_thermal_cylinder. Use list_component_types and get_component_type for thermal elements, then add_components, update_component, and connect. Use check_layout and snapshot to inspect the layout, and reroute or tidy to adjust it. Use run_thermal_analysis for steady and transient analysis, get_thermal_results for signals and samples, or export_thermal_csv. Edits are saved automatically. Use open_gui to show the live editor.

## Files

### `new_schematic`

Create a new empty Qucs-T .qucst file (overwrites only if overwrite=true).

| parameter | type | required | description |
|---|---|---|---|
| `path` | string | yes |  |
| `overwrite` | boolean |  |  |

### `open_schematic`

Open a Qucs-T .qucst file.

| parameter | type | required | description |
|---|---|---|---|
| `path` | string | yes |  |

### `save_schematic`

Save the schematic, optionally at a new path. Edits are saved automatically. To replace a different existing .qucst file pass overwrite=true.

| parameter | type | required | description |
|---|---|---|---|
| `path` | string |  |  |
| `overwrite` | boolean |  |  |

### `get_schematic`

Components (with absolute pin positions and nets), wires and nets of the open schematic.

| parameter | type | required | description |
|---|---|---|---|
| `include_wires` | boolean |  |  |

### `export_image`

Write the schematic as PNG or SVG (report style, no overlays) to a file.

| parameter | type | required | description |
|---|---|---|---|
| `path` | string | yes |  |
| `scale` | number |  | pixels per unit, default 2 |

## Thermal networks

### `new_thermal_schematic`

Create and open an empty Qucs-T .qucst file. Use list_component_types with category thermal, add_components, update_component and connect to edit it.

| parameter | type | required | description |
|---|---|---|---|
| `path` | string | yes |  |
| `overwrite` | boolean |  |  |

### `generate_thermal_cylinder`

Generate and open an editable Qucs-T cylinder including layers, contact resistance, surface conditions and time profiles. Uses the same generator as the GUI.

| parameter | type | required | description |
|---|---|---|---|
| `path` | string | yes |  |
| `overwrite` | boolean |  |  |
| `config` | object | yes |  |

### `get_thermal_settings`

Read saved analysis settings from the open thermal schematic.

### `set_thermal_settings`

Change analysis settings and save them in the open thermal schematic.

| parameter | type | required | description |
|---|---|---|---|
| `settings` | object | yes |  |

### `run_thermal_analysis`

Run the built-in Qucs-T steady or transient solver on the open .qucst. Optional settings override the saved settings for this run. Use get_thermal_results for values or export_thermal_csv for a complete CSV.

| parameter | type | required | description |
|---|---|---|---|
| `kind` | string | yes |  |
| `settings` | object |  |  |

### `get_thermal_results`

List available thermal signals and steady values, or read one signal by exact name, zero-based signal_index, or position_mm. Use at_s for interpolation, or offset and limit for raw samples.

| parameter | type | required | description |
|---|---|---|---|
| `group` | string |  |  |
| `signal` | string |  |  |
| `signal_index` | integer |  |  |
| `position_mm` | number[] |  | [axial position, radius] in mm; generated cylinders only |
| `at_s` | number[] |  |  |
| `offset` | integer |  |  |
| `limit` | integer |  |  |

### `export_thermal_csv`

Write all signals from the current thermal analysis to a new CSV file using UTF-8.

| parameter | type | required | description |
|---|---|---|---|
| `path` | string | yes | New .csv file; relative paths are resolved against QUCS_MCP_WORKDIR |

### `run_thermal_sweep`

Regenerate and analyze the saved cylinder configuration at each sweep value. Optionally report temperature at the same physical observation point for every case.

| parameter | type | required | description |
|---|---|---|---|
| `parameter` | string | yes |  |
| `from` | number | yes |  |
| `to` | number | yes |  |
| `count` | integer | yes |  |
| `kind` | string |  |  |
| `settings` | object |  |  |
| `position_mm` | number[] |  | [axial position, radius] in mm; generated cylinders only |

## Components

### `list_component_types`

Search thermal component types.

| parameter | type | required | description |
|---|---|---|---|
| `query` | string |  |  |
| `category` | string |  |  |

### `get_component_type`

Properties (name/default/description), pins and symbol size of a component type.

| parameter | type | required | description |
|---|---|---|---|
| `type` | string | yes |  |

### `add_components`

Place components. Returns their pins (absolute coordinates and lead direction) so you can plan wiring.

| parameter | type | required | description |
|---|---|---|---|
| `components` | object[] | yes |  |

### `update_component`

Change properties, orientation, position or name of a component (wires are NOT moved; use move_components for that).

| parameter | type | required | description |
|---|---|---|---|
| `name` | string | yes |  |
| `props` | object |  |  |
| `rotate` | integer |  |  |
| `mirror` | boolean |  |  |
| `x` | integer |  |  |
| `y` | integer |  |  |
| `new_name` | string |  |  |
| `mirror_y` | boolean |  | mirror about the Y axis (left-right) |
| `active` | integer |  | 1 active, 0 off |
| `label_offset` | integer[] |  | [tx, ty] text position relative to the component |

### `move_components`

Move components (absolute x/y or relative dx/dy) and re-route the wires attached to them.

| parameter | type | required | description |
|---|---|---|---|
| `moves` | object[] | yes |  |
| `reroute` | boolean |  | default true |

### `duplicate_components`

Copy components (and the wires between them) with an offset; names are renumbered. Returns the new names.

| parameter | type | required | description |
|---|---|---|---|
| `names` | string[] | yes |  |
| `dx` | integer |  |  |
| `dy` | integer |  |  |

### `align_components`

Align or distribute components: mode left|right|top|bottom|hcenter|vcenter|distribute_h|distribute_v. Wires are re-routed.

| parameter | type | required | description |
|---|---|---|---|
| `names` | string[] | yes |  |
| `mode` | string | yes |  |

### `delete_components`

Delete components (and optionally the wires of nets left without pins).

| parameter | type | required | description |
|---|---|---|---|
| `names` | string[] | yes |  |
| `delete_dangling_wires` | boolean |  |  |

## Wiring

### `connect`

Auto-route wires. Each entry of `nets` is a list of terminals that become one net ("Rth1.2", "[x,y]", "net:name"). Optional labels give the net a name.

| parameter | type | required | description |
|---|---|---|---|
| `nets` | string[][] | yes |  |
| `labels` | string[] |  | net label per entry of nets ("" for none) |
| `improve` | boolean |  | rip-up-and-retry pass to reduce crossings/bends (default true) |

### `add_wire`

Draw a wire through explicit points [[x,y],...] (diagonal steps are split horizontal-first).

| parameter | type | required | description |
|---|---|---|---|
| `points` | integer[][] | yes |  |

### `delete_wires`

Delete wires by index, by net name, or all.

| parameter | type | required | description |
|---|---|---|---|
| `indices` | integer[] |  |  |
| `nets` | string[] |  |  |
| `all` | boolean |  |  |

### `add_net_label`

Name a net (label placed automatically in free space). Same names connect nets, and the label identifies the node in temperature results.

| parameter | type | required | description |
|---|---|---|---|
| `target` | string | yes | pin, [x,y] or net:name |
| `label` | string | yes |  |

### `reroute`

Rip up and re-route the given nets (by name) or all nets. Use after rearranging parts or when wiring looks messy.

| parameter | type | required | description |
|---|---|---|---|
| `nets` | string[] |  |  |

### `tidy`

Clean up: snap to grid, merge/remove redundant wires, re-place labels, optionally re-route everything.

| parameter | type | required | description |
|---|---|---|---|
| `reroute` | boolean |  |  |
| `relabel` | boolean |  |  |

## Drawings

### `add_painting`

Add a drawing: kind line|rect|ellipse|arrow|text. line/arrow: (x,y)->(x+w,y+h); rect/ellipse: box; text: text at (x,y).

| parameter | type | required | description |
|---|---|---|---|
| `kind` | string | yes |  |
| `x` | integer | yes |  |
| `y` | integer | yes |  |
| `w` | integer |  |  |
| `h` | integer |  |  |
| `text` | string |  |  |
| `size` | integer |  |  |
| `color` | string |  | #rrggbb |
| `width` | integer |  |  |
| `filled` | boolean |  |  |

### `delete_paintings`

Delete drawings by index (get_schematic lists them) or all.

| parameter | type | required | description |
|---|---|---|---|
| `indices` | integer[] |  |  |
| `all` | boolean |  |  |

## Inspection

### `snapshot`

Render the schematic exactly as the editor draws it (PNG). Overlays help reasoning: coordinate grid+rulers, net colours and names, pin names, numbered issue markers.

| parameter | type | required | description |
|---|---|---|---|
| `region` | number[] |  | [x0,y0,x1,y1] in schematic units |
| `around` | string[] |  | zoom to these components |
| `grid` | boolean |  |  |
| `rulers` | boolean |  |  |
| `nets` | boolean |  |  |
| `pins` | boolean |  |  |
| `issues` | boolean |  |  |
| `highlight` | string[] |  | component names or net:name to highlight |
| `scale` | number |  | pixels per unit (default: fit into max_size) |
| `max_size` | integer |  | default 1400 |
| `clean` | boolean |  | report style: no grid, rulers or overlays |

### `check_layout`

Find problems: unconnected pins, pins/wire ends on the middle of wires (look connected but are not), overlapping parts or wires, wires through symbols, overlapping text, off-grid items. Also returns wiring metrics.

## Editor

### `open_gui`

Open the live editor window for the current schematic (it follows every change made here).

### `get_gui_state`

What the user has selected / is looking at in the editor window.
