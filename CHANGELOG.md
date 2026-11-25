# Changelog

## Unreleased

- Made the editor, CLI, and MCP server thermal-only.
- Use the `qucs-t` name for the MCP server and PDF producer metadata.
- Save conflict drafts as `.qucst` files.

## Qucs-T 0.1.0 (2026-11-24)

Qucs-T is a modified version of Qucs-S-Rust. It retains the inherited
GPL-2.0-or-later license and attribution for the inherited editor and symbol geometry.

- Added steady-state and transient thermal-network analysis, thermal schematic
  files, and heat balance and CSV results.
- Added multilayer cylinder generation, boundary schedules, temperature maps,
  parameter sweeps, and measured temperature overlays.
- Added thermal schematic generation and analysis to the stdio MCP server.
