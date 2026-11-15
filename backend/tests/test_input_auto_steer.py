import asyncio
import json
import os
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest

from app import api, pqueue, tmux
from app.adapters.codex.adapter import CodexAdapter
from app.adapters.codex import sessions


@pytest.fixture
def isolated(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sidecars")
    monkeypatch.setattr(api, "_session_exists", lambda name: True)
    monkeypatch.setattr(api, "_provider_of", lambda name: "codex")
    return tmp_path


class Rpc:
    closed = False

    def __init__(self, reject=False):
        self.calls = []
        self.reject = reject

    async def request(self, method, params):
        self.calls.append((method, params))
        if method == "turn/steer" and self.reject:
            raise RuntimeError("turno encerrado")
        return {}

    async def notifications(self):
        await asyncio.Event().wait()
        yield {}


def attach(monkeypatch, reject=False):
    adapter = CodexAdapter()
    rpc = Rpc(reject)
    adapter.attach("dest", rpc, "thread-1", subscribed=True)
    adapter._sessions["dest"].update(in_progress=True, turn_id="turn-1")
    monkeypatch.setattr(api, "get_adapter", lambda provider: adapter)
    return adapter, rpc



def _relatar_envio(source: str) -> str:
    """A função que imprime o resultado do envio: os recortes abaixo começam depois dela."""
    inicio = source.index("relatar_envio() {")
    return source[inicio:source.index("\n}\n", inicio) + 3]


@pytest.mark.parametrize("steered,delivered,expected", [
    (True, True, "entregue agora"), (False, True, "não orientado"), (False, False, "na fila"),
])
def test_cli_1a1_pede_steer_e_exibe_resultado(tmp_path, steered, delivered, expected):
    source = (Path(__file__).parents[2] / "scripts/hangar-send").read_text()
    # O `msg="$*"` com 4 espaços é o do envio normal; o mais indentado é o do par externo.
    tail = source[source.index('\n    msg="$*"') + 1:]
    # O recorte começa dentro do ramo do envio normal (o outro é o aviso de painel) e leva o `fi` dele.
    program = '''set -e
PY=python3
sender=origem
target=dest
sess=dest
set -- "recado"
api() { printf '%s' "$3" > "$BODY_FILE"; printf '%s' "$RESPONSE"; }
''' + _relatar_envio(source) + '''if true; then
''' + tail
    body_file = tmp_path / "body.json"
    result = subprocess.run(["bash", "-c", program], env={**os.environ,
        "BODY_FILE": str(body_file), "RESPONSE": json.dumps({"steered": steered, "delivered": delivered})},
        capture_output=True, text=True, check=True)
    assert json.loads(body_file.read_text()) == {"text": "[de: origem] recado", "steer": True}
    assert expected in result.stdout


def test_cli_claude_nativo_entrega_pelo_backend_sem_recusar(tmp_path):
    # O transporte é do backend: com socket nativo dos dois lados o script segue mandando por
    # /input e relata a entrega, nunca devolve o envio pro modelo fazer por outra ferramenta.
    source = (Path(__file__).parents[2] / "scripts/hangar-send").read_text()
    tail = source[source.index("forcar_tmux=0\n"):]
    program = '''set -e
PY=python3
set -- destino recado
api() { printf '%s\\n' "$1 $2" "$3" > "$BODY_FILE"; printf '%s' '{"ok": true, "delivered": true, "steered": true, "native": true}'; }
me() { echo origem; }
''' + _relatar_envio(source) + tail
    body_file = tmp_path / "body.txt"
    result = subprocess.run(["bash", "-c", program], env={**os.environ, "BODY_FILE": str(body_file)},
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    chamada, corpo = body_file.read_text().splitlines()
    assert chamada == "POST /api/sessions/destino/input"
    assert json.loads(corpo) == {"text": "[de: origem] recado", "steer": True}
    assert "entregue agora -> destino (no socket do Claude dele" in result.stdout
