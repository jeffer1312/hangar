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


@pytest.mark.parametrize("reject", [False, True])
async def test_codex_auto_steer_preserva_uma_entrada_ate_completar(isolated, monkeypatch, reject):
    adapter, rpc = attach(monkeypatch, reject)
    text = "[de: origem] ajuste o pedido"
    result = await asyncio.wait_for(api.input_prompt("dest", api.InputBody(text=text, steer=True)), 2)
    assert result["steered"] is not reject
    calls = [params for method, params in rpc.calls if method == "turn/steer"]
    assert len(calls) == 1
    assert calls[0]["expectedTurnId"] == "turn-1"
    assert calls[0]["input"] == [{"type": "text", "text": text}]
    queue = pqueue.PromptQueue("dest")
    assert len(queue.load()) == 1
    assert queue.load()[0]["delivered"] is not reject
    adapter._sessions["dest"]["in_progress"] = False
    send = AsyncMock(return_value="sent")
    monkeypatch.setattr(adapter, "send_prompt", send)
    assert await adapter.drain("dest", "unused") == int(reject)
    assert await adapter.drain("dest", "unused") == 0
    assert send.await_count == int(reject)
    assert len(pqueue.PromptQueue("dest").load()) == 1
    rollout = isolated / "rollout.jsonl"
    rollout.write_text(json.dumps({"type": "response_item", "timestamp": datetime.now(timezone.utc).isoformat(),
        "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}}) + "\n")
    history = pqueue.merged_history("dest", str(rollout), provider="codex")
    assert len([event for event in history if event.kind == "user_msg" and event.text == text]) == 1


async def test_codex_sem_steer_preserva_composer(isolated, monkeypatch):
    adapter, rpc = attach(monkeypatch)
    result = await api.input_prompt("dest", api.InputBody(text="pedido normal"))
    assert result == {"ok": True, "delivered": False, "steered": False}
    assert rpc.calls == []
    assert len(pqueue.PromptQueue("dest").load()) == 1


async def test_auto_steer_nao_antecipa_pedido_do_composer(isolated, monkeypatch):
    adapter, rpc = attach(monkeypatch)
    await api.input_prompt("dest", api.InputBody(text="pedido normal"))
    result = await api.input_prompt("dest", api.InputBody(text="recado", steer=True))
    assert result["steered"] is True
    assert [params["input"][0]["text"] for method, params in rpc.calls if method == "turn/steer"] == ["recado"]
    rows = pqueue.PromptQueue("dest").load()
    assert [(row["text"], row["delivered"]) for row in rows] == [("pedido normal", False), ("recado", True)]
    adapter._sessions["dest"]["in_progress"] = False
    send = AsyncMock(return_value="sent")
    monkeypatch.setattr(adapter, "send_prompt", send)
    assert await adapter.drain("dest", "unused") == 1
    send.assert_awaited_once_with("dest", "pedido normal")


async def test_recados_concorrentes_confirmam_seus_proprios_ids(isolated, monkeypatch):
    _, rpc = attach(monkeypatch)
    original = api._send_one_codex
    ready = asyncio.Event()
    count = 0

    async def persist_both(*args, **kwargs):
        nonlocal count
        result = await original(*args, **kwargs)
        count += 1
        if count == 2:
            ready.set()
        await ready.wait()
        return result

    monkeypatch.setattr(api, "_send_one_codex", persist_both)
    results = await asyncio.wait_for(asyncio.gather(*[
        api.input_prompt("dest", api.InputBody(text=text, steer=True)) for text in ("A", "B")
    ]), 2)
    assert results == [{"ok": True, "delivered": True, "steered": True}] * 2
    assert sorted(params["input"][0]["text"] for method, params in rpc.calls if method == "turn/steer") == ["A", "B"]


async def test_orientacao_confirmada_sobrevive_a_falha_posterior_do_lote(isolated, monkeypatch):
    adapter, rpc = attach(monkeypatch)
    queue = pqueue.PromptQueue("dest")
    original_send = api._send_one_codex
    original_request = rpc.request

    async def fail_second(method, params):
        if method == "turn/steer" and params["input"][0]["text"] == "B":
            raise RuntimeError("turno encerrou")
        return await original_request(method, params)

    async def manual_steer_wins(*args, **kwargs):
        result = await original_send(*args, **kwargs)
        queue.append("B")
        with pytest.raises(RuntimeError, match="turno encerrou"):
            await adapter.steer_queue("dest")
        return result

    monkeypatch.setattr(rpc, "request", fail_second)
    monkeypatch.setattr(api, "_send_one_codex", manual_steer_wins)
    result = await api.input_prompt("dest", api.InputBody(text="A", steer=True))
    assert result == {"ok": True, "delivered": True, "steered": True}
    assert [params["input"][0]["text"] for method, params in rpc.calls if method == "turn/steer"] == ["A"]
    reloaded = pqueue.PromptQueue("dest")
    assert [(row["text"], row["delivered"]) for row in reloaded.load()] == [("A", True), ("B", False)]
    assert reloaded.reconcile_delivered(set(), 0, time.time() + 100) == []
    assert reloaded.load()[0]["delivered"] is True


async def test_falha_ao_gravar_recibo_preserva_orientacao_aceita(isolated, monkeypatch, caplog):
    _, rpc = attach(monkeypatch)
    original = pqueue.PromptQueue.set_delivered

    def fail_receipt(self, entry_id, value, **kwargs):
        if kwargs.get("steered"):
            raise OSError("disco indisponível")
        return original(self, entry_id, value, **kwargs)

    monkeypatch.setattr(pqueue.PromptQueue, "set_delivered", fail_receipt)
    result = await api.input_prompt("dest", api.InputBody(text="A", steer=True))
    assert result == {"ok": True, "delivered": True, "steered": True}
    assert len([method for method, _ in rpc.calls if method == "turn/steer"]) == 1
    assert pqueue.PromptQueue("dest").load()[0]["delivered"] is True
    assert "recibo indisponivel" in caplog.text


async def test_falha_ao_reler_recibo_preserva_orientacao_aceita(isolated, monkeypatch, caplog):
    adapter, _ = attach(monkeypatch)
    original = adapter.steer_queue

    async def fail_read_after_accept(*args, **kwargs):
        sent = await original(*args, **kwargs)
        monkeypatch.setattr(pqueue.PromptQueue, "load", Mock(side_effect=PermissionError("sem leitura")))
        return sent

    monkeypatch.setattr(adapter, "steer_queue", fail_read_after_accept)
    result = await api.input_prompt("dest", api.InputBody(text="A", steer=True))
    assert result == {"ok": True, "delivered": True, "steered": True}
    assert "recibo ilegivel" in caplog.text


async def test_codex_confere_id_do_recado_e_nao_sucesso_de_outra_entrada(isolated, monkeypatch):
    adapter, _ = attach(monkeypatch)
    monkeypatch.setattr(adapter, "steer_queue", AsyncMock(return_value=["outro-id"]))
    result = await api.input_prompt("dest", api.InputBody(text="recado", steer=True))
    assert result["steered"] is False
    assert len(pqueue.PromptQueue("dest").load()) == 1


async def test_codex_drain_que_vence_corrida_nao_gera_segundo_envio(isolated, monkeypatch):
    adapter, rpc = attach(monkeypatch)
    original = adapter.steer_queue
    send = AsyncMock(return_value="sent")
    monkeypatch.setattr(adapter, "send_prompt", send)

    async def completed_before_steer(name, **kwargs):
        adapter._sessions[name]["in_progress"] = False
        await adapter.drain(name, "unused")
        return await original(name, **kwargs)

    monkeypatch.setattr(adapter, "steer_queue", completed_before_steer)
    result = await api.input_prompt("dest", api.InputBody(text="recado", steer=True))
    assert result["steered"] is False
    assert send.await_count == 1
    assert result["delivered"] is True
    assert not any(method == "turn/steer" for method, _ in rpc.calls)
    assert len(pqueue.PromptQueue("dest").load()) == 1
    assert await adapter.drain("dest", "unused") == 0


async def test_codex_sem_persistencia_nao_tenta_orientar(isolated, monkeypatch):
    _, rpc = attach(monkeypatch)
    monkeypatch.setattr(pqueue.PromptQueue, "append", Mock(side_effect=OSError("disco indisponível")))
    with pytest.raises(api.HTTPException) as failure:
        await api.input_prompt("dest", api.InputBody(text="recado", steer=True))
    assert failure.value.status_code == 400
    assert rpc.calls == []


@pytest.mark.parametrize("provider", ["kimi", "claude", "pi", "omp"])
async def test_kimi_promove_mesmo_entregue_e_outros_nao_reenviam(isolated, monkeypatch, provider):
    monkeypatch.setattr(api, "_provider_of", lambda name: "claude")
    monkeypatch.setattr(api, "_pane_info", lambda name: (provider, "%test"))
    monkeypatch.setattr(api.pi_inbox, "linha_de", lambda *args: object() if provider in ("pi", "omp") else None)
    monkeypatch.setattr(tmux, "list_panes_active", lambda: [])
    monkeypatch.setattr(api.registry, "resolve_tracked", lambda *args: (None, False))
    monkeypatch.setattr(api.threading, "Timer", lambda *args, **kwargs: SimpleNamespace(start=lambda: None))
    send = Mock(return_value="sent")
    steer = Mock(return_value=True)
    monkeypatch.setattr(api.terminal, "send_prompt", send)
    monkeypatch.setattr(api.terminal_input, "steer_now", steer)
    result = await api.input_prompt("dest", api.InputBody(text="recado", steer=True))
    assert result["delivered"] is True
    assert result["steered"] is (provider == "kimi")
    assert send.call_count == 1
    assert steer.call_count == int(provider == "kimi")
    queue = pqueue.PromptQueue("dest")
    assert len(queue.load()) == 1
    if provider == "kimi":
        assert queue.load()[0]["confirmed"] is True
        assert queue.reconcile_delivered(set(), 0, time.time() + 100) == []
        assert pqueue.PromptQueue("dest").claim_undelivered() == []


@pytest.mark.parametrize("promoted", [False, "sem-fila"])
async def test_kimi_sem_promocao_mantem_recado(isolated, monkeypatch, promoted):
    queue = pqueue.PromptQueue("dest")
    entry = queue.append("recado", delivered=True)
    monkeypatch.setattr(api, "_provider_of", lambda name: "claude")
    monkeypatch.setattr(api, "_pane_info", lambda name: ("kimi", "%test"))
    monkeypatch.setattr(api, "_send_one", lambda *args: {"ok": True, "delivered": True, "entry_id": entry["id"]})
    monkeypatch.setattr(api.terminal_input, "steer_now", lambda name: promoted)
    result = await api.input_prompt("dest", api.InputBody(text="recado", steer=True))
    assert result["steered"] is False
    assert len(queue.load()) == 1
    assert not queue.load()[0].get("confirmed")


@pytest.mark.parametrize("steered,delivered,expected", [
    (True, True, "entregue agora"), (False, True, "não orientado"), (False, False, "na fila"),
])
def test_cli_1a1_pede_steer_e_exibe_resultado(tmp_path, steered, delivered, expected):
    source = (Path(__file__).parents[2] / "scripts/hangar-send").read_text()
    tail = source[source.index('text="[de: $sender] $*"'):]
    program = '''set -e
sender=origem
target=dest
sess=dest
set -- "recado"
api() { printf '%s' "$3" > "$BODY_FILE"; printf '%s' "$RESPONSE"; }
''' + tail
    body_file = tmp_path / "body.json"
    result = subprocess.run(["bash", "-c", program], env={**os.environ,
        "BODY_FILE": str(body_file), "RESPONSE": json.dumps({"steered": steered, "delivered": delivered})},
        capture_output=True, text=True, check=True)
    assert json.loads(body_file.read_text()) == {"text": "[de: origem] recado", "steer": True}
    assert expected in result.stdout


def test_cli_claude_nativo_continua_recusando_envio_por_input():
    source = (Path(__file__).parents[2] / "scripts/hangar-send").read_text()
    tail = source[source.index("forcar_tmux=0\n"):]
    program = '''set -e
set -- destino recado
api() { printf '%s' '{"uds":"/tmp/inbox"}'; }
''' + tail
    result = subprocess.run(["bash", "-c", program], env={**os.environ,
        "CLAUDE_CODE_MESSAGING_SOCKET": "/tmp/origem"}, capture_output=True, text=True)
    assert result.returncode == 3
    assert "SendMessage" in result.stderr
    assert "entregue" not in result.stdout
