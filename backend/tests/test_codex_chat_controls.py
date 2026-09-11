"""Controles do chat usam a thread viva e entradas estruturadas do Codex."""
import pytest
import asyncio
import shutil

from app.adapters.codex.adapter import CodexAdapter
from app.adapters.codex import sessions


class Client:
    closed = False

    def __init__(self):
        self.calls = []
        self.fail = None

    async def request(self, method, params):
        self.calls.append((method, params))
        if self.fail == method:
            raise RuntimeError("turno encerrado")
        if method == "thread/read":
            return {"thread": {"model": "gpt-6-astra", "reasoningEffort": "high"}}
        if method == "skills/list":
            return {"data": [{"skills": [
                {"name": "revisar", "path": "/skills/revisar/SKILL.md", "enabled": True},
                {"name": "oculta", "path": "/skills/oculta/SKILL.md", "enabled": False},
            ]}]}
        return {}


@pytest.fixture
def chat(tmp_path, monkeypatch):
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sidecars")
    client = Client()
    adapter = CodexAdapter()
    adapter.attach("sess", client, "thread-1", model="antigo", effort="medium", subscribed=True)
    return adapter, client


async def test_leitura_viva_vence_esforco_antigo(chat):
    adapter, _ = chat
    assert await adapter.read_settings("sess") == {
        "model": "gpt-6-astra", "effort": "high", "mode": None,
    }


async def test_plan_preserva_modelo_esforco_e_permissoes(chat):
    adapter, client = chat
    await adapter.set_mode("sess", "plan")
    method, params = client.calls[-1]
    assert method == "thread/settings/update"
    assert params == {"threadId": "thread-1", "collaborationMode": {
        "mode": "plan", "settings": {"model": "gpt-6-astra", "reasoning_effort": "high",
                                     "developer_instructions": None}}}
    assert (await adapter.read_settings("sess"))["mode"] == "plan"


async def test_skill_usa_identidade_nativa_e_mantem_texto_para_historico(chat):
    adapter, client = chat
    await adapter.send_prompt("sess", "/revisar confira o diff")
    method, params = client.calls[-1]
    assert method == "turn/start"
    assert params["input"] == [{"type": "text", "text": "/revisar confira o diff"},
                               {"type": "skill", "name": "revisar", "path": "/skills/revisar/SKILL.md"}]
    assert "model" not in params and "effort" not in params
    assert [s["name"] for s in await adapter.list_skills("sess")] == ["revisar"]


async def test_orientar_exige_turno_atual_e_nao_inicia_outro(chat):
    adapter, client = chat
    adapter._sessions["sess"].update(turn_id="turno-1", in_progress=True)
    await adapter.steer("sess", "corrija isso")
    assert client.calls[-1] == ("turn/steer", {
        "threadId": "thread-1", "expectedTurnId": "turno-1",
        "input": [{"type": "text", "text": "corrija isso"}],
    })
    client.fail = "turn/steer"
    with pytest.raises(RuntimeError):
        await adapter.steer("sess", "outra orientação")
    assert all(method != "turn/start" for method, _ in client.calls)


async def test_reabrir_stream_recupera_turno_e_permite_orientar(chat):
    adapter, client = chat
    snapshot = {"thread": {"id": "thread-1", "status": {"type": "active", "activeFlags": []},
                           "turns": [{"id": "anterior", "status": "completed"},
                                     {"id": "atual", "status": "inProgress"}]}}
    original = client.request

    async def request(method, params):
        result = await original(method, params)
        if method != "thread/read":
            return result
        thread = dict(snapshot["thread"])
        if not params["includeTurns"]:
            thread["turns"] = []
        return {"thread": thread}

    async def notifications():
        await asyncio.Event().wait()
        yield {}

    client.request, client.notifications = request, notifications
    stream = adapter.state_monitor("sess", lambda: "thread-1")
    try:
        first = await anext(stream)
        assert first.state == "working"
        assert client.calls[0] == ("thread/read", {"threadId": "thread-1", "includeTurns": False})
        assert adapter._sessions["sess"].get("turn_id") is None
        await adapter.steer("sess", "ajuste durante o turno")
        assert client.calls[-2] == ("thread/read", {"threadId": "thread-1", "includeTurns": True})
        assert client.calls[-1][1]["expectedTurnId"] == "atual"
        assert await adapter.deliverable("sess") is False
        second = adapter.state_monitor("sess", lambda: "thread-1")
        try:
            assert (await anext(second)).state == "working"
            assert [call for call in client.calls if call[0] == "thread/read"][-1] == (
                "thread/read", {"threadId": "thread-1", "includeTurns": False})
            assert client.calls[-1] == ("account/rateLimits/read", {})
            assert adapter._sessions["sess"]["turn_id"] == "atual"
        finally:
            await second.aclose()
    finally:
        await stream.aclose()


async def test_resposta_de_leitura_antiga_nao_reabre_turno_concluido(chat):
    adapter, client = chat
    async def notifications():
        yield {"method": "turn/completed", "params": {"threadId": "thread-1", "turn": {"id": "atual"}}}
    client.notifications = notifications

    async def request(method, params):
        await adapter._consumir("sess", client, adapter._sessions["sess"], lambda event: None)
        return {"thread": {"status": {"type": "active"},
                           "turns": [{"id": "atual", "status": "inProgress"}]}}

    client.request = request
    await adapter.read_settings("sess", include_turns=True)
    sess = adapter._sessions["sess"]
    assert sess["state"] == "idle" and not sess["in_progress"] and sess["turn_id"] is None
    with pytest.raises(RuntimeError, match="Não há turno"):
        await adapter.steer("sess", "orientação tardia")


async def test_assinatura_recupera_turno_sem_notificacao_de_inicio(chat):
    adapter, client = chat
    adapter._sessions["sess"]["subscribed"] = False
    async def request(method, params):
        return {"thread": {"status": {"type": "active"},
                           "turns": [{"id": "atual", "status": "inProgress"}]}}
    client.request = request
    await adapter._subscribe_when_ready("sess", "/tmp")
    assert adapter._sessions["sess"]["turn_id"] == "atual"
    assert await adapter.deliverable("sess") is False


async def test_conexao_recupera_turno_antes_de_aceitar_envio(chat, monkeypatch):
    from app.adapters.codex import adapter as module
    adapter, client = chat
    async def connect(endpoint):
        return endpoint
    calls = []
    async def request(method, params):
        calls.append((method, params))
        turns = ([{"id": "atual", "status": "inProgress"}]
                 if method == "thread/read" and params["includeTurns"] else [])
        return {"thread": {"status": {"type": "active"}, "turns": turns}}
    client.connect, client.request = connect, request
    monkeypatch.setattr(module, "AppServerClient", lambda: client)
    monkeypatch.setattr(module, "pid_vivo", lambda pid: True)
    monkeypatch.setattr(adapter, "_start_tmux_watcher", lambda name: None)
    monkeypatch.setattr(adapter, "start_subscription", lambda name, cwd: None)
    await adapter._conectar("sess", {"thread_id": "thread-1", "app_pid": 123, "endpoint": "ws://fake"})
    assert calls[1] == ("thread/read", {"threadId": "thread-1", "includeTurns": False})
    assert adapter._sessions["sess"].get("turn_id") is None
    assert await adapter.deliverable("sess") is False
    await adapter.steer("sess", "ajuste")
    assert calls[-2] == ("thread/read", {"threadId": "thread-1", "includeTurns": True})


async def test_modelo_rejeitado_nao_altera_estado(chat):
    adapter, client = chat
    client.fail = "thread/settings/update"
    with pytest.raises(RuntimeError):
        await adapter.set_model("sess", "outro", "low")
    assert adapter.current_model("sess")["model"] == "antigo"


async def test_notificacao_terminal_atualiza_modo_e_esforco(chat):
    adapter, client = chat
    async def notifications():
        yield {"method": "thread/settings/updated", "params": {
            "threadId": "thread-1", "threadSettings": {"model": "gpt-5.6-sol", "effort": "xhigh",
                                                       "collaborationMode": {"mode": "plan"}}}}
    client.notifications = notifications
    events = []
    await adapter._consumir("sess", client, adapter._sessions["sess"], events.append)
    assert events[-1].codex_mode == "plan"
    assert "xhigh" in events[-1].status_line
    assert adapter.current_model("sess") == {"model": "gpt-5.6-sol", "effort": "xhigh"}


async def test_eventos_de_outra_thread_nao_alteram_sessao_principal(chat, monkeypatch):
    from app.adapters.codex.adapter import CodexPreviewSource
    adapter, client = chat
    sess = adapter._sessions["sess"]
    sess.update(state="working", in_progress=True, turn_id="turno-main")
    preview = CodexPreviewSource.get("sess")
    await preview.push("prévia principal")
    drained = []
    async def drain(*args):
        drained.append(args)
        return 0
    monkeypatch.setattr(adapter, "drain", drain)
    async def notifications():
        for method, params in [
            ("thread/status/changed", {"status": {"type": "idle"}}),
            ("turn/started", {"turn": {"id": "turno-sub"}}),
            ("item/agentMessage/delta", {"delta": "texto subagente"}),
            ("turn/completed", {"turn": {"id": "turno-sub"}}),
            ("thread/tokenUsage/updated", {"tokenUsage": {"last": {"totalTokens": 999}}}),
        ]:
            yield {"method": method, "params": {"threadId": "outra-thread", **params}}
        yield {"method": "account/rateLimits/updated", "params": {"rateLimits": {"primary": {"usedPercent": 12}}}}
    client.notifications = notifications
    events = []
    await adapter._consumir("sess", client, sess, events.append)
    assert (sess["state"], sess["in_progress"], sess["turn_id"]) == ("working", True, "turno-main")
    assert preview.text == "prévia principal"
    assert drained == [] and "token_usage" not in sess
    assert len(events) == 1 and events[0].state == "working"
    assert sess["rate_limits"] == {"primary": {"usedPercent": 12}}


async def test_nova_principal_religa_adapter_e_ignora_subagente(chat, monkeypatch):
    from app.adapters.codex import adapter as module
    adapter, old = chat
    sessions.save("sess", "thread-1", "/old.jsonl", "/p", model="modelo", effort="high",
                  endpoint="ws://fake", app_pid=123)
    previous = adapter._sessions["sess"]
    previous.update(endpoint="ws://fake", app_pid=123, mode="plan", turn_id="old-turn")
    main = {"id": "new", "path": "/new.jsonl", "cwd": "/p", "source": "vscode", "threadSource": "user"}
    async def notifications():
        for thread in [{**main, "id": "sub", "source": {"subAgent": {}}}, main]:
            yield {"method": "thread/started", "params": {"thread": thread}}
    async def close():
        old.closed = True
    old.notifications, old.close = notifications, close
    await adapter._consumir("sess", old, previous, lambda event: None)
    assert sessions.load("sess")["thread_id"] == "new"
    new = Client()
    async def connect(endpoint):
        return endpoint
    async def request(method, params):
        return {"thread": {**main, "status": {"type": "active"}, "turns": [{"id": "new-turn", "status": "inProgress"}]}}
    new.connect, new.request = connect, request
    monkeypatch.setattr(module, "AppServerClient", lambda: new)
    monkeypatch.setattr(module, "pid_vivo", lambda pid: True)
    monkeypatch.setattr(adapter, "_start_tmux_watcher", lambda name: None)
    subscriptions = []
    monkeypatch.setattr(adapter, "start_subscription", lambda *args: subscriptions.append(args))
    assert await adapter.ensure_running("sess") is new
    assert old.closed and subscriptions == [("sess", "/p")]
    assert adapter._sessions["sess"]["thread_id"] == "new"
    assert adapter._sessions["sess"].get("turn_id") is None
    assert adapter._sessions["sess"]["in_progress"] is True
    assert adapter._sessions["sess"].get("mode") is None
    await adapter._consumir("sess", old, previous, lambda event: pytest.fail("evento antigo publicado"))
    assert adapter._sessions["sess"]["client"] is new


def test_troca_thread_preserva_modelo_e_recusa_escritor_atrasado(chat):
    from concurrent.futures import ThreadPoolExecutor
    sessions.save("sess", "old", "/old.jsonl", "/p", model="m", effort="high",
                  endpoint="ws://fake", app_pid=123)
    main = {"id": "new", "path": "/new.jsonl", "cwd": "/p", "source": "vscode"}
    with ThreadPoolExecutor(max_workers=2) as workers:
        switched = workers.submit(sessions.switch_thread, "sess", main, "old", endpoint="ws://fake", app_pid=123)
        updated = workers.submit(sessions.update_model, "sess", "novo-modelo", "low")
        assert switched.result()
        updated.result()
    assert not sessions.switch_thread("sess", {**main, "id": "atrasada"}, "old", endpoint="ws://fake", app_pid=123)
    assert not sessions.switch_thread("sess", {**main, "id": "outro-dono"}, "new", endpoint="ws://fake", app_pid=456)
    meta = sessions.load("sess")
    assert (meta["thread_id"], meta["rollout_path"], meta["model"], meta["effort"]) == ("new", "/new.jsonl", "novo-modelo", "low")


@pytest.mark.parametrize("operation", ["rename", "delete"])
def test_troca_thread_serializa_com_rename_e_delete(chat, monkeypatch, operation):
    from concurrent.futures import ThreadPoolExecutor
    from contextlib import contextmanager
    from threading import Event, local
    sessions.save("sess", "old", "/old.jsonl", "/p", model="m", effort="high",
                  endpoint="ws://fake", app_pid=123)
    before = {**sessions.load("sess"), "extra": "preservado"}
    sessions._write("sess", before)
    writing, competing = Event(), Event()
    role = local()
    write, locked = sessions._write, sessions._locked

    def paused_write(name, meta):
        if name == "sess" and meta["thread_id"] == "new":
            writing.set()
            assert competing.wait(5)
        write(name, meta)

    @contextmanager
    def observed_lock(*names):
        if getattr(role, "operation", False):
            competing.set()
        with locked(*names):
            yield

    def mutate():
        role.operation = True
        if operation == "rename":
            sessions.rename("sess", "renamed")
        else:
            sessions.delete("sess")

    monkeypatch.setattr(sessions, "_write", paused_write)
    monkeypatch.setattr(sessions, "_locked", observed_lock)
    with ThreadPoolExecutor(max_workers=2) as workers:
        switched = workers.submit(sessions.switch_thread, "sess",
            {"id": "new", "path": "/new.jsonl", "cwd": "/p", "source": "vscode"},
            "old", endpoint="ws://fake", app_pid=123)
        assert writing.wait(5)
        changed = workers.submit(mutate)
        assert switched.result(timeout=5)
        changed.result(timeout=5)
    assert sessions.load("sess") is None
    assert sessions.load("renamed") == ({**before, "name": "renamed", "thread_id": "new",
                                        "rollout_path": "/new.jsonl"} if operation == "rename" else None)


async def test_falha_orientacao_restaura_fila(chat, monkeypatch, tmp_path):
    from app import pqueue
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    adapter, client = chat
    adapter._sessions["sess"].update(turn_id="turno-1", in_progress=True)
    client.fail = "turn/steer"
    queue = pqueue.PromptQueue("sess")
    queue.append("mensagem pendente")
    with pytest.raises(RuntimeError):
        await adapter.steer_queue("sess")
    assert queue.load()[0]["delivered"] is False


async def test_orientar_fila_devolve_apenas_ids_confirmados(chat, monkeypatch, tmp_path):
    from app import pqueue
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    adapter, client = chat
    adapter._sessions["sess"].update(turn_id="turno-1", in_progress=True)
    queue = pqueue.PromptQueue("sess")
    queue.append("já entregue", delivered=True)
    first = queue.append("primeira orientação")
    second = queue.append("segunda orientação")
    assert await adapter.steer_queue("sess") == [first["id"], second["id"]]
    assert [params["input"][0]["text"] for method, params in client.calls
            if method == "turn/steer"] == ["primeira orientação", "segunda orientação"]
    assert await adapter.steer_queue("sess") == []


@pytest.mark.skipif(not shutil.which("codex"), reason="Codex CLI necessário para validar o protocolo")
async def test_controles_no_codex_real_sem_inferencia(tmp_path, monkeypatch):
    from app.adapters.codex.appserver import AppServerClient
    (tmp_path / ".codex").mkdir()
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / ".codex"))
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sidecars")
    client = AppServerClient()
    reconnected = AppServerClient()
    adapter = CodexAdapter()
    settings_updates: asyncio.Queue = asyncio.Queue()
    try:
        await client.start_shared()
        await client.request("initialize", {"clientInfo": {"name": "hangar-test", "version": "1"},
                                            "capabilities": {"experimentalApi": True}})
        result = await client.request("thread/start", {"cwd": str(tmp_path), "model": "gpt-6-astra", "ephemeral": True})
        notifications = client.notifications

        async def observar_notifications():
            async for notification in notifications():
                if notification.get("method") == "thread/settings/updated":
                    settings_updates.put_nowait(notification)
                yield notification

        client.notifications = observar_notifications
        adapter.attach("native", client, result["thread"]["id"], subscribed=True)
        await adapter.set_model("native", "gpt-6-astra", "high")
        assert (await adapter.read_settings("native"))["effort"] == "high"
        await adapter.set_mode("native", "plan")
        async with asyncio.timeout(10):
            while True:
                notification = await settings_updates.get()
                if notification.get("method") == "thread/settings/updated":
                    settings = notification["params"]["threadSettings"]
                    if settings["collaborationMode"]["mode"] == "plan":
                        assert settings["effort"] == "high"
                        assert settings["sandboxPolicy"]["type"] == "readOnly"
                        break
        await reconnected.connect(client.endpoint)
        await reconnected.request("initialize", {"clientInfo": {"name": "hangar-reconnected", "version": "1"},
                                                 "capabilities": {"experimentalApi": True}})
        fresh_adapter = CodexAdapter()
        fresh_adapter.attach("native", reconnected, result["thread"]["id"], subscribed=True)
        assert (await fresh_adapter.read_settings("native"))["mode"] is None
        await fresh_adapter.set_mode("native", "default")
        assert (await fresh_adapter.read_settings("native"))["mode"] == "default"
        with pytest.raises(RuntimeError):
            await adapter.steer("native", "sem turno")
    finally:
        await reconnected.close()
        await client.close()
