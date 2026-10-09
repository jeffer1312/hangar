"""Mede CPU (ms/s) do Python e do hangar-server e eventos SSE de uma sessão, por estado.

Uso: medir-claude-sem-terminal.py PID_PYTHON PID_RUST SESSAO SEGUNDOS SAIDA.json [ENV] [URL]
  ENV: .env com CP_AUTH_TOKEN (padrão: backend/.env deste repositório)
  URL: base do backend (padrão: http://127.0.0.1:8765)
"""
import json, os, sys, threading, time, urllib.request

PY, RS, NAME, SECS, OUT = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], int(sys.argv[4]), sys.argv[5]
ENV = sys.argv[6] if len(sys.argv) > 6 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "backend", ".env")
BASE = (sys.argv[7] if len(sys.argv) > 7 else "http://127.0.0.1:8765").rstrip("/")
TOKEN = next(l.split("=", 1)[1].strip() for l in open(ENV) if l.startswith("CP_AUTH_TOKEN="))
TICK = os.sysconf("SC_CLK_TCK")
state = {"now": "?"}
events = []  # (t, tipo, bytes)


def cpu(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return (int(f[11]) + int(f[12])) * 1000 / TICK  # utime+stime em ms


def sse():
    req = urllib.request.Request(f"{BASE}/api/sessions/{NAME}/events",
                                 headers={"Authorization": f"Bearer {TOKEN}", "Accept": "text/event-stream"})
    kind, size = "message", 0
    with urllib.request.urlopen(req, timeout=SECS + 30) as r:
        for raw in r:
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            if line.startswith("event:"):
                kind = line[6:].strip()
            elif line.startswith("data:"):
                size += len(line)
                if kind == "state":
                    try:
                        state["now"] = json.loads(line[5:]).get("state", state["now"])
                    except ValueError:
                        pass
            elif line == "":
                events.append((time.monotonic(), kind, size))
                kind, size = "message", 0


threading.Thread(target=sse, daemon=True).start()
time.sleep(2)
start = time.monotonic()
samples = []
prev = (cpu(PY), cpu(RS), time.monotonic())
while time.monotonic() - start < SECS:
    time.sleep(1)
    cur = (cpu(PY), cpu(RS), time.monotonic())
    dt = cur[2] - prev[2]
    samples.append({"t": cur[2], "state": state["now"], "py": (cur[0] - prev[0]) / dt, "rs": (cur[1] - prev[1]) / dt})
    prev = cur

out = {}
for st in sorted({s["state"] for s in samples}):
    win = [s for s in samples if s["state"] == st]
    secs = len(win)
    kinds = {}
    for t, k, b in events:
        # Evento conta no estado do segundo em que chegou.
        owner = next((s for s in samples if s["t"] - 1 <= t < s["t"]), None)
        if owner and owner["state"] == st:
            kinds.setdefault(k, [0, 0]); kinds[k][0] += 1; kinds[k][1] += b
    out[st] = {"segundos": secs, "python_ms_s": round(sum(s["py"] for s in win) / secs, 1),
               "rust_ms_s": round(sum(s["rs"] for s in win) / secs, 1),
               "eventos_por_s": {k: round(v[0] / secs, 2) for k, v in kinds.items()},
               "bytes_por_s": {k: round(v[1] / secs) for k, v in kinds.items()}}
json.dump(out, open(OUT, "w"), indent=1, ensure_ascii=False)
print(json.dumps(out, ensure_ascii=False))
