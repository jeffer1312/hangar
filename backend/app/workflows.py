import json
import re
from pathlib import Path

# Lê os artefatos de Workflow no disco pra um painel "estilo /workflows do terminal": lista de runs
# + detalhe (fases + agentes com state/tokens/tempo). Dois lugares por run (mesmo runId):
#   <session-dir>/workflows/wf_<runId>.json        -> resumo de CONCLUSÃO (rico: workflowProgress[])
#   <session-dir>/subagents/workflows/wf_<runId>/  -> dir VIVO (journal.jsonl + agent-*.jsonl)
# session-dir = o jsonl do transcript sem a extensão (<slug>/<uuid>.jsonl -> <slug>/<uuid>/).


def _session_dir(jsonl: str) -> Path:
    p = Path(jsonl)
    return p.parent / p.stem


def _script_name(sd: Path, run_id: str) -> str | None:
    # workflows/scripts/<name>-wf_<runId>.js -> pega o meta.name (ou o prefixo do arquivo).
    scripts = sd / "workflows" / "scripts"
    if not scripts.is_dir():
        return None
    for f in scripts.glob(f"*{run_id}.js"):
        try:
            txt = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        m = re.search(r"name:\s*['\"]([^'\"]+)['\"]", txt)
        if m:
            return m.group(1)
        # fallback: <name>-wf_... -> <name>
        base = f.name
        cut = base.find("-wf_")
        if cut > 0:
            return base[:cut]
    return None


def _agent_tokens(rundir: Path, agent_id: str) -> int:
    # Soma os usos de token do transcript do subagente (agent-<id>.jsonl).
    f = rundir / f"agent-{agent_id}.jsonl"
    if not f.is_file():
        return 0
    total = 0
    try:
        for line in f.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                o = json.loads(line)
            except (json.JSONDecodeError, ValueError):
                continue
            usage = (o.get("message") or {}).get("usage") or {}
            for k in ("input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"):
                v = usage.get(k)
                if isinstance(v, int):
                    total += v
    except OSError:
        return 0
    return total


def _live_agents(rundir: Path) -> list[dict]:
    # Estado por agente a partir do journal.jsonl: started-sem-result = rodando; com result = feito.
    jf = rundir / "journal.jsonl"
    started: list[str] = []
    resulted: set[str] = set()
    if jf.is_file():
        try:
            for line in jf.read_text(encoding="utf-8", errors="replace").splitlines():
                try:
                    ev = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                aid = ev.get("agentId")
                if not aid:
                    continue
                if ev.get("type") == "started" and aid not in started:
                    started.append(aid)
                elif ev.get("type") == "result":
                    resulted.add(aid)
        except OSError:
            pass
    agents = []
    for aid in started:
        agents.append({
            "label": aid[:8],
            "phaseTitle": None,
            "state": "done" if aid in resulted else "progress",
            "model": None,
            "tokens": _agent_tokens(rundir, aid),
            "durationMs": 0,
            "toolCalls": 0,
            "lastToolName": None,
            "lastToolSummary": None,
            "resultPreview": None,
        })
    return agents


def _summary_from_completion(o: dict, run_id: str) -> dict:
    return {
        "runId": o.get("runId") or run_id,
        "name": o.get("workflowName") or run_id,
        "status": o.get("status") or "completed",
        "agentCount": o.get("agentCount") or 0,
        "phaseCount": len(o.get("phases") or []),
        "totalTokens": o.get("totalTokens") or 0,
        "durationMs": o.get("durationMs") or 0,
        "startTime": o.get("startTime") or 0,
        "running": False,
    }


def list_workflows(jsonl: str) -> list[dict]:
    sd = _session_dir(jsonl)
    out: dict[str, dict] = {}

    wfdir = sd / "workflows"
    if wfdir.is_dir():
        for f in wfdir.glob("wf_*.json"):
            try:
                o = json.loads(f.read_text(encoding="utf-8", errors="replace"))
            except (OSError, json.JSONDecodeError, ValueError):
                continue
            rid = o.get("runId") or f.stem
            out[rid] = _summary_from_completion(o, rid)

    rundir = sd / "subagents" / "workflows"
    if rundir.is_dir():
        for d in rundir.glob("wf_*"):
            if not d.is_dir() or d.name in out:
                continue  # tem completion json -> já listado como concluído
            agents = _live_agents(d)
            out[d.name] = {
                "runId": d.name,
                "name": _script_name(sd, d.name) or d.name,
                "status": "running",
                "agentCount": len(agents),
                "phaseCount": 0,
                "totalTokens": sum(a["tokens"] for a in agents),
                "durationMs": 0,
                "startTime": 0,
                "running": True,
            }

    rows = list(out.values())
    # rodando primeiro; depois mais recentes (startTime desc).
    rows.sort(key=lambda r: (not r["running"], -int(r.get("startTime") or 0)))
    return rows


def get_workflow(jsonl: str, run_id: str) -> dict | None:
    sd = _session_dir(jsonl)

    f = sd / "workflows" / f"{run_id}.json"
    if f.is_file():
        try:
            o = json.loads(f.read_text(encoding="utf-8", errors="replace"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None
        phases = [{"title": p.get("title"), "detail": p.get("detail")} for p in (o.get("phases") or [])]
        agents = []
        for row in (o.get("workflowProgress") or []):
            if row.get("type") != "workflow_agent":
                continue
            agents.append({
                "label": row.get("label"),
                "phaseTitle": row.get("phaseTitle"),
                "state": row.get("state"),
                "model": row.get("model"),
                "tokens": row.get("tokens") or 0,
                "durationMs": row.get("durationMs") or 0,
                "toolCalls": row.get("toolCalls") or 0,
                "lastToolName": row.get("lastToolName"),
                "lastToolSummary": row.get("lastToolSummary"),
                "resultPreview": row.get("resultPreview"),
            })
        return {
            "runId": o.get("runId") or run_id,
            "name": o.get("workflowName") or run_id,
            "status": o.get("status") or "completed",
            "totalTokens": o.get("totalTokens") or 0,
            "durationMs": o.get("durationMs") or 0,
            "summary": o.get("summary"),
            "phases": phases,
            "agents": agents,
        }

    rundir = sd / "subagents" / "workflows" / run_id
    if rundir.is_dir():
        agents = _live_agents(rundir)
        return {
            "runId": run_id,
            "name": _script_name(sd, run_id) or run_id,
            "status": "running",
            "totalTokens": sum(a["tokens"] for a in agents),
            "durationMs": 0,
            "summary": None,
            "phases": [],
            "agents": agents,
        }
    return None
