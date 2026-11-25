"""Live-sync check: start the GUI on a file, edit it through the MCP server, the GUI must follow."""
import json, subprocess, sys, time, os
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
EXE = Path(os.environ["QUCS_RS_EXE"]) if os.environ.get("QUCS_RS_EXE") else ROOT / "target" / "debug" / "qucs-t.exe"
OUT = ROOT / "target" / "e2e"
p = subprocess.Popen([str(EXE), "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env={**os.environ, "QUCS_MCP_WORKDIR": str(OUT)})
n = 0
def call(name, **a):
    global n
    n += 1
    p.stdin.write((json.dumps({"jsonrpc": "2.0", "id": n, "method": "tools/call", "params": {"name": name, "arguments": a}}) + "\n").encode()); p.stdin.flush()
    r = json.loads(p.stdout.readline())["result"]
    t = [c["text"] for c in r["content"] if c["type"] == "text"]
    if r["isError"]: raise SystemExit(t)
    return t[0] if t else None
call("new_thermal_schematic", path="live", overwrite=True)
call("open_gui")
time.sleep(float(sys.argv[1]) if len(sys.argv) > 1 else 4)
call("add_components", components=[{"type": "ThPower", "name": "P1", "x": 100, "y": 200},
                                   {"type": "ThResistance", "name": "Rth1", "x": 220, "y": 140},
                                   {"type": "ThBoundary", "name": "T1", "x": 340, "y": 200}])
call("connect", nets=[["P1.1", "Rth1.1"], ["Rth1.2", "T1.1"]], labels=["hot", "ambient"])
time.sleep(2)
print(call("get_gui_state"))
p.stdin.close(); p.wait(5)
