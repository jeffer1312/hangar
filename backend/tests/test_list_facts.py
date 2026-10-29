"""Fatos da lista que continuam do Python: só o estado das linhas Pi, omp e Kimi, que o
Rust pergunta quando a lista as tem (`POST /internal/list/facts`)."""
import asyncio
import json

from app import hook_state, list_facts, registry, sse


def _row(name, provider="claude", **extra):
    return {"name": name, "provider": provider, "state": "idle", **extra}


def _compute(rows, pane_pids=None):
    return asyncio.run(list_facts.compute(rows, pane_pids or {}))


def test_facts_compute_only_pi_omp_kimi_rows(monkeypatch):
    seen = []

    async def fake_state(infos, state_only=False):
        seen.append(([i.name for i in infos], state_only))
        for i in infos:
            i.state, i.label = "working", f"rótulo {i.name}"
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_state)
    out = _compute([_row("cc"), _row("cx", "codex"), _row("pp", "pi"), _row("hl", headless=True)])
    # Claude e Codex são do Rust: nem chegam ao classificador do Python.
    assert seen == [(["pp"], True)]
    assert out == {"states": {"pp": out["states"]["pp"]}}
    assert (out["states"]["pp"]["state"], out["states"]["pp"]["label"]) == ("working", "rótulo pp")
    json.dumps(out)


def test_facts_preserve_rust_accounts_for_kimi_pi_omp_rows(monkeypatch):
    async def keep(infos, state_only=False):
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", keep)
    out = _compute([_row("k", "kimi", conta="kimi:apikey"),
                    _row("p", "pi", jsonl="/t/p.jsonl", conta="chave:kimi-coding"),
                    _row("o", "omp")], pane_pids={"p": 41})
    assert set(out["states"]) == {"k", "p", "o"}
    assert all("conta" not in state for state in out["states"].values())


def test_demote_service_updates_map_and_file(tmp_path, monkeypatch):
    state_dir = tmp_path / ".hangar-state"
    state_dir.mkdir()
    sidecar = state_dir / "sid1.json"
    sidecar.write_text(json.dumps({"state": "awaiting_input", "ts": 5.0}))
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    monkeypatch.setattr(hook_state, "hook_state", hs)
    assert hs.get_state("sid1")[0] == "awaiting_input"
    for sid in ["sid1", "ausente"]:
        hs.demote_awaiting(sid)
    assert hs.get_state("sid1") == ("idle", 5.0)
    assert json.loads(sidecar.read_text()) == {"state": "idle", "ts": 5.0}


