"""Exercise the Qucs-T thermal workflow over the real stdio MCP transport."""

import csv
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    default_exe = "qucs-t.exe" if os.name == "nt" else "qucs-t"
    exe = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target" / "debug" / default_exe
    if os.name == "nt" and not exe.suffix:
        exe = exe.with_suffix(".exe")
    with tempfile.TemporaryDirectory() as temp:
        requests = []

        def request(method, params):
            msg = {"jsonrpc": "2.0", "id": len(requests) + 1, "method": method, "params": params}
            requests.append(msg)
            return msg["id"]

        def call(tool_name, **args):
            return request("tools/call", {"name": tool_name, "arguments": args})

        initialized = request("initialize", {"protocolVersion": "2025-06-18"})
        listing = request("tools/list", {})
        created = call("new_thermal_schematic", path="rc")
        catalog = call("list_component_types")
        rejected_legacy_path = call("open_schematic", path="legacy.sch")
        rejected_legacy_header = call("open_schematic", path="legacy.qucst")
        rejected_electrical_part = call("open_schematic", path="mixed.qucst")
        unavailable = [
            call(name) for name in ("run_simulation", "get_netlist", "dc_operating_point", "search_library")
        ]
        call("add_components", components=[
            {"type": "ThResistance", "name": "R1", "x": 0, "y": 0, "props": {"R [K/W]": 2}},
            {"type": "ThCapacity", "name": "C1", "x": -30, "y": 30, "props": {"C [J/K]": 10}},
            {"type": "ThPower", "name": "P1", "x": -90, "y": 30, "props": {"定常発熱 [W]": 5}},
            {"type": "ThBoundary", "name": "T1", "x": 30, "y": 30, "props": {"温度 [°C]": 25}},
        ])
        call("connect", nets=[["P1.1", "R1.1"]], labels=["chip"])
        call("set_thermal_settings", settings={"end_s": 20, "output_step_s": 1})
        settings = call("get_thermal_settings")
        call("run_thermal_analysis", kind="steady")
        steady = call("get_thermal_results", group="temperature", signal="chip")
        call("run_thermal_analysis", kind="transient")
        transient = call("get_thermal_results", group="temperature", signal="chip", at_s=[0, 20, 21])
        saved = call("export_thermal_csv", path="rc-results")
        repeated = call("export_thermal_csv", path="rc-results.csv")
        rejected = call("add_components", components=[{"type": "ThCapacity", "name": "Bad"}, {"type": "R"}])
        after_rejection = call("get_schematic", include_wires=False)
        query_after_rejection = call("get_thermal_results", group="temperature", signal="chip", limit=1)
        call("update_component", name="P1", props={"定常発熱 [W]": 10})
        stale = call("get_thermal_results")
        call("open_schematic", path="rc.qucst")
        call("run_thermal_analysis", kind="steady")
        updated = call("get_thermal_results", group="temperature", signal="chip", limit=1)
        generated = call("generate_thermal_cylinder", path="cylinder", config={
            "length_mm": 100,
            "inner_mm": 5,
            "axial": 3,
            "layers": [{
                "outer_mm": 10, "radial": 2,
                "material": {"name": "steel", "conductivity": 15, "density": 7850, "specific_heat": 480},
            }, {
                "outer_mm": 12, "radial": 1, "contact_to_inner": 0.0001,
                "material": {"name": "cover", "conductivity": 2, "density": 1200, "specific_heat": 700},
            }],
            "surfaces": {
                "end_a": {"kind": "Power", "power": 10, "power_profile": {"enabled": True, "rows": "0,0\n1,10"}},
                "outer": {"kind": "Convection", "h": 10, "temperature": 25},
            },
        })
        cylinder_steady = call("run_thermal_analysis", kind="steady")
        point = call("get_thermal_results", position_mm=[50, 6])
        sweep = call("run_thermal_sweep", parameter="outer_convection", **{"from": 10, "to": 20}, count=2, position_mm=[50, 6])

        legacy_schematic = "<Qucs Schematic 26.1.1>\n"
        for name in ("legacy.sch", "legacy.qucst"):
            (Path(temp) / name).write_text(legacy_schematic, encoding="utf-8")
        (Path(temp) / "mixed.qucst").write_text(
            "<Qucs-T Schematic 1>\n<Components>\n<R R1 1 0 0 0 0 0 0>\n</Components>\n",
            encoding="utf-8",
        )
        env = dict(os.environ, QUCS_MCP_WORKDIR=temp)
        payload = "".join(json.dumps(msg, ensure_ascii=False) + "\n" for msg in requests)
        process = subprocess.run([exe, "mcp"], cwd=root, env=env, input=payload,
                                 encoding="utf-8", capture_output=True, timeout=30, check=True)
        messages = {item["id"]: item for item in map(json.loads, process.stdout.splitlines())}
        assert set(messages) == set(range(1, len(requests) + 1))
        for request_id in unavailable:
            assert messages[request_id]["error"]["code"] == -32602
        responses = {key: message["result"] for key, message in messages.items() if key not in unavailable}
        expected_errors = {repeated, rejected, stale, rejected_legacy_path, rejected_legacy_header, rejected_electrical_part}
        for msg in requests:
            if msg["id"] in unavailable:
                continue
            response = responses[msg["id"]]
            if msg["method"] == "tools/call":
                assert response["isError"] == (msg["id"] in expected_errors), (msg, response)

        def output(call_id):
            return json.loads(responses[call_id]["content"][0]["text"])

        assert responses[initialized]["serverInfo"]["name"] == "qucs-t"
        tool_names = {tool["name"] for tool in responses[listing]["tools"]}
        assert "run_thermal_analysis" in tool_names
        assert not {"run_simulation", "get_netlist", "dc_operating_point", "search_library"} & tool_names
        assert output(created)["format"] == "qucst"
        assert output(catalog)["count"] == 9
        assert {part["category"] for part in output(catalog)["types"]} == {"thermal"}
        assert output(settings)["end_s"] == 20
        assert abs(output(steady)["samples"][0]["value"] - 35) < 1e-8
        samples = output(transient)["samples"]
        assert abs(samples[0]["value"] - 25) < 1e-8
        assert abs(samples[1]["value"] - 31.3212) < 0.04
        assert samples[2]["value"] is None
        path = Path(output(saved)["path"])
        with path.open(encoding="utf-8-sig", newline="") as f:
            assert "chip [°C]" in next(csv.reader(f))
        assert all(comp["name"] != "Bad" for comp in output(after_rejection)["components"])
        assert output(query_after_rejection)["total_samples"] > 1
        assert "stale" in responses[stale]["content"][0]["text"]
        assert abs(output(updated)["samples"][0]["value"] - 45) < 1e-8
        assert "外面_区間1" in output(generated)["named_nets"]
        assert abs(output(cylinder_steady)["peak_release_w"] - 10) < 1e-6
        assert output(point)["samples"][0]["value"] > 25
        rows = output(sweep)["rows"]
        assert rows[0]["peak_temperature_c"] > rows[1]["peak_temperature_c"]
        assert rows[0]["observed_peak_c"] is not None and rows[1]["observed_peak_c"] is not None
    print("Thermal MCP workflow passed")


if __name__ == "__main__":
    main()
