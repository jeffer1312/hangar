"""Origem com terminal da transferência para o Codex saindo do pane (ordem: o pane morre antes do
sidecar, o que passa de um lado pro outro, a volta quando o sidecar não grava) e a recusa da troca
de modo, que é do Rust."""
import asyncio
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from fastapi.testclient import TestClient

from app.adapters.claude_headless import sessions as S
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
from app.config import settings

SID = "11111111-1111-1111-1111-111111111111"


def _tm(tmp_path):
    """O que a transferência gravou da origem; bate com o pane de `_pane`."""
    return {"session_id": SID, "cwd": str(tmp_path), "pane_pid": 999, "key": "k-t1", "transfer_id": "tr-1"}


@pytest.fixture
def reg(tmp_path, monkeypatch):
    from app import pqueue, registry as R
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(S, "_trocando", {})
    monkeypatch.setattr(R.procinfo, "pid_vivo", lambda pid: False)
    monkeypatch.setattr(R.tmux, "has_session", lambda n: False)
    return R.SessionRegistry(str(tmp_path / "projects"))


@pytest.mark.parametrize("resume", [False, True])
def test_terminal_command_keeps_plugins_and_choices(monkeypatch, resume):
    import shlex
    from app import plugin_bridge, registry as R
    roots = ["/plugins/hangar", "/plugins/outro mod"]
    monkeypatch.setattr(plugin_bridge, "raizes_dos_plugins", lambda: roots)
    meta = {"session_id": SID, "cwd": "/projeto", "model": "haiku", "effort": "low",
            "permission_mode": "acceptEdits"}
    argv = shlex.split(R.SessionRegistry._comando_terminal(meta, resume=resume))
    assert argv[:3] == ["claude", "--resume" if resume else "--session-id", SID]
    assert [argv[i + 1] for i, value in enumerate(argv[:-1]) if value == "--plugin-dir"] == roots
    assert argv[argv.index("--model") + 1] == "haiku"
    assert argv[argv.index("--effort") + 1] == "low"
    assert argv[argv.index("--permission-mode") + 1] == "acceptEdits"


def _pane(reg, tmp_path, monkeypatch, *, mata=True):
    from app import registry as R
    jsonl = str(tmp_path / "projects" / "x" / f"{SID}.jsonl")
    monkeypatch.setattr(reg, "_pane_of", lambda n: {"name": n, "cwd": str(tmp_path), "pid": 999})
    monkeypatch.setattr(R.SessionRegistry, "_refuse_non_claude_resume", staticmethod(lambda p, **_: False))
    monkeypatch.setattr(reg, "resolve_tracked", lambda n, c: (jsonl, True))
    monkeypatch.setattr(R, "_config_dir_of", lambda pid: tmp_path / ".claude-b")
    monkeypatch.setattr(R, "_engine_of", lambda pid: None)
    monkeypatch.setattr(R.procinfo, "_model_of", lambda pid: ("sonnet", "high"))
    monkeypatch.setattr(R.procinfo, "_env_var_of",
                        lambda pid, n: "haiku" if n == "CLAUDE_CODE_SUBAGENT_MODEL" else None)
    monkeypatch.setattr(R, "_descendant_pids", lambda pid: [999, 1000])
    monkeypatch.setattr(R, "agente_do_pane", lambda pid, children=None: ("claude", 999))
    monkeypatch.setattr(R, "_escolhas_status", lambda sid: (None, None))
    mortos = []

    def kill(n):
        mortos.append((n, S.exists(n)))
        return mata
    monkeypatch.setattr(R.tmux, "kill_session", kill)
    return mortos


def test_monitor_sem_terminal_nao_diz_dead_durante_a_troca(tmp_path, monkeypatch):
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(S, "_trocando", {})
    S.marcar_troca("hl")
    ad = ClaudeHeadlessAdapter()

    async def fluxo():
        gen = ad.state_monitor("hl", lambda: None)
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(gen.__anext__(), 1.5)
        S._trocando.clear()
        gen = ad.state_monitor("hl", lambda: None)
        assert (await gen.__anext__()).state == "dead"
    asyncio.run(fluxo())


@pytest.fixture
def cliente(monkeypatch):
    import app.api as api_mod
    settings.auth_token = "secret"
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    return TestClient(api_mod.app)


_H = {"Authorization": "Bearer secret"}


@pytest.mark.parametrize("provider", ["claude", "codex", "pi"])
def test_mode_switch_is_rust_only(cliente, provider):
    from app.models import SessionInfo
    info = SessionInfo(name="s1", cwd="/tmp", provider=provider)
    with patch("app.api._cached_info", AsyncMock(return_value=info)):
        r = cliente.post("/api/sessions/s1/modo-execucao", headers=_H, json={"terminal": True})
    if provider == "pi":
        assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_modo_so_claude"
    else:
        assert r.status_code == 503 and r.json()["detail"]["code"] == "session_open_needs_rust_server"
