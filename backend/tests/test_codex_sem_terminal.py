"""Codex sem terminal: app-server (falso, em stdio) atrás do cano real, sem pane."""
import json
import os
import sys
import time
import uuid
from pathlib import Path
from unittest.mock import patch

import pytest

from app import tmux
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex import sem_terminal
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex.adapter import CodexAdapter

pytestmark = pytest.mark.skipif(os.name == "nt", reason="socket unix")

# `codex app-server --stdio` de mentira: initialize (uma vez), thread/start, turn/start que pede
# aprovação de comando, thread/read.
_CODEX_FALSO = r'''#!/usr/bin/env python3
import json, os, sys
with open("server-starts.jsonl", "a") as recorded:
    recorded.write(json.dumps(sys.argv[1:]) + "\n")
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
iniciado = False
status = "idle"
effort = None
service_tier = next((json.loads(arg.split("=", 1)[1]) for arg in sys.argv[1:]
                     if arg.startswith("service_tier=")), None)
if os.environ.get("FAKE_TIER_REFUSED"):
    service_tier = "default"
for linha in sys.stdin:
    ev = json.loads(linha)
    m = ev.get("method")
    if m == "initialize":
        if iniciado:
            out({"jsonrpc": "2.0", "id": ev["id"], "error": {"code": -32600, "message": "Already initialized"}})
        else:
            iniciado = True
            out({"jsonrpc": "2.0", "id": ev["id"], "result": {"userAgent": "falso"}})
    elif m == "thread/start":
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {"thread": {"id": "th-1", "path": ""}, "model": "gpt-falso", "serviceTier": service_tier}})
    elif m == "thread/resume":
        with open("resume.txt", "w") as f:
            f.write(json.dumps(ev["params"]))
        with open("resume-history.jsonl", "a") as f:
            f.write(json.dumps(ev["params"]) + "\n")
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {"thread": {"id": ev["params"]["threadId"]}, "model": "gpt-falso", "serviceTier": service_tier}})
    elif m == "thread/settings/update":
        # O modo colaborativo também carrega o esforço; ajuste sem o campo não o apaga.
        mode = (ev["params"].get("collaborationMode") or {}).get("settings") or {}
        effort = ev["params"].get("effort", mode.get("reasoning_effort", effort))
        with open("settings.txt", "w") as f:
            f.write(json.dumps(ev["params"]))
        if ev["params"].get("effort") == "recusado":
            out({"jsonrpc": "2.0", "id": ev["id"], "error": {"code": -32602, "message": "effort invalido"}})
        else:
            service_tier = ev["params"].get("serviceTier", service_tier)
            out({"jsonrpc": "2.0", "id": ev["id"], "result": {}})
            out({"jsonrpc": "2.0", "method": "thread/settings/updated", "params": {
                "threadId": "th-1", "threadSettings": {"serviceTier": service_tier}}})
    elif m == "thread/read":
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {"thread": {"id": "th-1", "model": "gpt-falso", "reasoningEffort": effort, "status": {"type": status}, "turns": []}}})
    elif m == "turn/start":
        status = "active"
        with open("turno.txt", "w") as f:
            f.write(json.dumps(ev["params"]))
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {"turn": {"id": "t-1"}}})
        out({"jsonrpc": "2.0", "method": "turn/started", "params": {"threadId": "th-1", "turn": {"id": "t-1"}}})
        out({"jsonrpc": "2.0", "id": 0, "method": "item/commandExecution/requestApproval",
             "params": {"threadId": "th-1", "turnId": "t-1", "itemId": "exec-1", "command": "touch x",
                        "cwd": "/tmp", "reason": "fora do sandbox"}})
    elif m is None and ev.get("id") == 0:
        with open("decisao.txt", "w") as f:
            f.write(json.dumps(ev))
        out({"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "th-1", "requestId": 0}})
        out({"jsonrpc": "2.0", "id": 9, "method": "mcpServer/elicitation/request",
             "params": {"threadId": "th-1", "serverName": "x", "message": "?"}})
    elif m is None and ev.get("id") == 9:
        with open("elicitacao.txt", "w") as f:
            f.write(json.dumps(ev))
        status = "idle"
        out({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th-1", "turn": {"id": "t-1", "status": "completed"}}})
    else:
        out({"jsonrpc": "2.0", "id": ev.get("id"), "result": {}})
'''


@pytest.fixture
def ambiente(tmp_path, monkeypatch):
    binario = tmp_path / "bin"
    binario.mkdir()
    fake = binario / "codex"
    fake.write_text(_CODEX_FALSO)
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", f"{binario}{os.pathsep}{os.environ['PATH']}")
    pasta = tmp_path / "codex-sessions"
    fila = tmp_path / "fila"
    fila.mkdir()
    from app import pqueue
    with patch.object(codex_sessions, "_dir", lambda: pasta), \
         patch.object(pqueue, "_queue_dir", lambda: fila), \
         patch.object(tmux, "_scope_prefix", lambda: []):
        yield tmp_path


def _sidecar(nome: str, cwd: Path, **escolha) -> dict:
    codex_sessions.save(nome, None, "", str(cwd), headless=True, key=uuid.uuid4().hex,
                        permission_mode="Ask for approval", **escolha)
    return codex_sessions.load(nome)


def test_modo_padrao_e_full_access():
    assert sem_terminal.MODO_PADRAO == "Full Access"
    assert sem_terminal.politica(None) == ("never", "danger-full-access")
    assert sem_terminal.modos_para_tela(None)["current"] == "Full Access"


def test_ambiente_identifica_a_sessao_headless_para_os_scripts(ambiente, monkeypatch):
    meta = _sidecar("cx-identidade", ambiente)
    monkeypatch.setenv("TMUX", "herdado")
    monkeypatch.setenv("TMUX_PANE", "%9")

    env = sem_terminal._ambiente(meta)

    assert env["CP_SESSION_NAME"] == "cx-identidade"
    assert env["CP_SESSION_KEY"] == meta["key"]
    assert env["HANGAR_CANO_KEY"] == meta["key"]
    assert env["HANGAR_CANO_OWNER"] == str(Path.home())
    assert "TMUX" not in env and "TMUX_PANE" not in env


def test_identidade_vai_ao_shell_do_codex_por_set(ambiente):
    # `set` vale com qualquer `inherit` do usuário (provado no codex-cli 0.160.1 com inherit="core");
    # aqui fica o contrato: cada `-c` é TOML válido e cai em shell_environment_policy.set.<VAR>.
    import tomllib
    meta = _sidecar("cx-shell", ambiente)
    argv = sem_terminal.argv(meta)
    prefixo = "shell_environment_policy.set."
    sets = {}
    for flag, valor in zip(argv, argv[1:]):
        if flag == "-c" and valor.startswith(prefixo):
            chave, toml = valor.split("=", 1)
            sets[chave.removeprefix(prefixo)] = tomllib.loads(f"v = {toml}")["v"]
    assert sets == {"CP_SESSION_NAME": "cx-shell", "CP_SESSION_KEY": meta["key"], "HANGAR_CANO_KEY": meta["key"]}


def test_politica_por_modo():
    assert sem_terminal.politica("Ask for approval") == ("on-request", "read-only")
    assert sem_terminal.politica("Full Access") == ("never", "danger-full-access")
    assert sem_terminal.politica(None) == ("never", "danger-full-access")
    # S-05: texto fora da lista é sidecar estragado; abrir em Full Access por engano é pior que recusar.
    with pytest.raises(ValueError):
        sem_terminal.politica("qualquer coisa")
    assert '-c' in sem_terminal.argv({"permission_mode": "Full Access"})
    assert 'approval_policy="never"' in sem_terminal.argv({"permission_mode": "Full Access"})


def test_stdio_budget_is_optional_and_validated():
    argv = sem_terminal.argv({"permission_mode": "Full Access", "tool_output_token_limit": 144000})
    assert argv[:3] == ["codex", "app-server", "--stdio"]
    assert "tool_output_token_limit=144000" in argv
    assert not any("tool_output_token_limit" in arg for arg in sem_terminal.argv({}))
    with pytest.raises(ValueError):
        sem_terminal.argv({"tool_output_token_limit": True})
