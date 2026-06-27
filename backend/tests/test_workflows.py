"""Cobertura do parser de Workflow (workflows.py): list/get_workflow/get_agent a partir dos
artefatos em disco (completion json + journal/agent jsonl do dir vivo)."""
import json
from pathlib import Path

from app import workflows


def _sd(tmp_path: Path):
    # jsonl = <proj>/<uuid>.jsonl -> session_dir = <proj>/<uuid>/
    jsonl = tmp_path / "proj" / "sess.jsonl"
    jsonl.parent.mkdir(parents=True, exist_ok=True)
    jsonl.write_text("", encoding="utf-8")
    sd = jsonl.parent / "sess"
    sd.mkdir()
    return str(jsonl), sd


def _write(p: Path, obj):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(obj if isinstance(obj, str) else json.dumps(obj), encoding="utf-8")


def _write_lines(p: Path, objs):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("".join(json.dumps(o) + "\n" for o in objs), encoding="utf-8")


def test_list_workflows_empty(tmp_path):
    jsonl, _ = _sd(tmp_path)
    assert workflows.list_workflows(jsonl) == []


def test_list_workflows_completed(tmp_path):
    jsonl, sd = _sd(tmp_path)
    _write(sd / "workflows" / "wf_abc.json", {
        "runId": "wf_abc", "workflowName": "review", "status": "completed",
        "agentCount": 3, "phases": [{"title": "P1"}, {"title": "P2"}],
        "totalTokens": 1200, "durationMs": 5000, "startTime": 100,
    })
    rows = workflows.list_workflows(jsonl)
    assert len(rows) == 1
    r = rows[0]
    assert r["runId"] == "wf_abc" and r["name"] == "review" and r["status"] == "completed"
    assert r["agentCount"] == 3 and r["phaseCount"] == 2 and r["totalTokens"] == 1200
    assert r["running"] is False


def test_list_workflows_running_with_script_name(tmp_path):
    jsonl, sd = _sd(tmp_path)
    rd = sd / "subagents" / "workflows" / "wf_run"
    _write_lines(rd / "journal.jsonl", [
        {"type": "started", "agentId": "a1deadbeef"},
        {"type": "started", "agentId": "a2cafef00d"},
        {"type": "result", "agentId": "a1deadbeef"},
    ])
    _write_lines(rd / "agent-a1deadbeef.jsonl", [
        {"message": {"model": "claude-opus", "role": "assistant",
                     "usage": {"input_tokens": 10, "output_tokens": 5},
                     "content": [{"type": "tool_use", "name": "Read"},
                                 {"type": "text", "text": "pensei"}]}},
    ])
    _write(sd / "workflows" / "scripts" / "myflow-wf_run.js", "export const meta = { name: 'myflow' }")
    r = workflows.list_workflows(jsonl)[0]
    assert r["running"] is True and r["status"] == "running"
    assert r["name"] == "myflow" and r["agentCount"] == 2
    assert r["totalTokens"] == 15  # a1: 10+5; a2 sem transcript -> 0


def test_list_workflows_running_sorts_first(tmp_path):
    jsonl, sd = _sd(tmp_path)
    _write(sd / "workflows" / "wf_done.json", {"runId": "wf_done", "startTime": 999})
    _write_lines(sd / "subagents" / "workflows" / "wf_live" / "journal.jsonl",
                 [{"type": "started", "agentId": "x1"}])
    rows = workflows.list_workflows(jsonl)
    assert [r["runId"] for r in rows] == ["wf_live", "wf_done"]  # rodando primeiro


def test_get_workflow_completed(tmp_path):
    jsonl, sd = _sd(tmp_path)
    _write(sd / "workflows" / "wf_abc.json", {
        "runId": "wf_abc", "workflowName": "rev", "status": "completed",
        "totalTokens": 50, "durationMs": 10, "summary": "ok",
        "phases": [{"title": "P", "detail": "d"}],
        "workflowProgress": [
            {"type": "workflow_phase", "title": "P"},
            {"type": "workflow_agent", "agentId": "a1", "label": "review", "state": "done",
             "tokens": 50, "toolCalls": 2, "lastToolName": "Read"},
        ],
    })
    wf = workflows.get_workflow(jsonl, "wf_abc")
    assert wf is not None and wf["name"] == "rev" and wf["summary"] == "ok"
    assert wf["phases"] == [{"title": "P", "detail": "d"}]
    assert len(wf["agents"]) == 1 and wf["agents"][0]["agentId"] == "a1"
    assert wf["agents"][0]["tokens"] == 50


def test_get_workflow_running(tmp_path):
    jsonl, sd = _sd(tmp_path)
    _write_lines(sd / "subagents" / "workflows" / "wf_live" / "journal.jsonl",
                 [{"type": "started", "agentId": "z9"}])
    wf = workflows.get_workflow(jsonl, "wf_live")
    assert wf is not None and wf["status"] == "running"
    assert len(wf["agents"]) == 1 and wf["agents"][0]["state"] == "progress"


def test_get_workflow_missing(tmp_path):
    jsonl, _ = _sd(tmp_path)
    assert workflows.get_workflow(jsonl, "wf_nope") is None


def test_get_agent_completion_journal_and_transcript(tmp_path):
    jsonl, sd = _sd(tmp_path)
    _write(sd / "workflows" / "wf_abc.json", {
        "workflowProgress": [
            {"type": "workflow_agent", "agentId": "a1", "label": "rev", "state": "done",
             "tokens": 99, "model": "opus", "phaseTitle": "P"},
        ],
    })
    rd = sd / "subagents" / "workflows" / "wf_abc"
    _write_lines(rd / "journal.jsonl", [{"type": "result", "agentId": "a1", "result": {"verdict": "ok"}}])
    _write_lines(rd / "agent-a1.jsonl", [
        {"type": "user", "message": {"content": "faz a coisa"}},
        {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Read"}, {"type": "tool_use", "name": "Read"},
            {"type": "tool_use", "name": "Grep"}]}},
    ])
    a = workflows.get_agent(jsonl, "wf_abc", "a1")
    assert a is not None
    assert a["label"] == "rev" and a["model"] == "opus" and a["tokens"] == 99
    assert a["prompt"] == "faz a coisa"
    assert '"verdict": "ok"' in a["result"]
    assert {t["name"]: t["count"] for t in a["tools"]} == {"Read": 2, "Grep": 1}


def test_get_agent_missing(tmp_path):
    jsonl, _ = _sd(tmp_path)
    assert workflows.get_agent(jsonl, "wf_x", "nope") is None
