"""Coordenação real com processos/RPC simulados; não é prova do runtime nativo."""
import asyncio
from dataclasses import replace
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock
import uuid

import pytest
from fastapi.testclient import TestClient
from pydantic import ValidationError

from app import api, conversation_transfer as store, pqueue, registry as registry_module
from app.adapters import CLAUDE_HEADLESS
from app.adapters.claude_headless import sessions as hs
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter
from app.adapters.codex import sessions as cs, transfer as importer
from app.adapters.codex.adapter import CodexAdapter
from app.models import SessionInfo


@pytest.fixture
def scenario(tmp_path, monkeypatch):
    monkeypatch.setattr(store, "_base", lambda: tmp_path / "transfers")
    monkeypatch.setattr(hs, "_dir", lambda: tmp_path / "headless")
    monkeypatch.setattr(cs, "_dir", lambda: tmp_path / "codex-sidecars")
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path / "queue")
    (tmp_path / "queue").mkdir()
    sid = str(uuid.uuid4())
    source = tmp_path / ".claude" / "projects" / "project" / f"{sid}.jsonl"
    source.parent.mkdir(parents=True)
    source.write_text(json.dumps({"type": "user", "uuid": "u1", "parentUuid": None,
                                  "message": {"role": "user", "content": "contexto exclusivo"}}) + "\n")
    original = source.read_bytes()
    meta = hs.save("s", str(tmp_path), sid, config_dir=str(tmp_path / ".claude"),
                   key="source-key", permission_mode="manual")
    info = SessionInfo(name="s", cwd=str(tmp_path), jsonl=str(source), tracked=True,
                       provider="claude", headless=True, conta=f"claude:{tmp_path / '.claude'}")
    reg = registry_module.SessionRegistry()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(reg, "_forget", Mock())
    monkeypatch.setattr(registry_module.tmux, "has_session", lambda name: False)
    monkeypatch.setattr(store, "session_life", lambda name: "k:source-key")
    hl, cx = ClaudeHeadlessAdapter(), CodexAdapter()
    live = SimpleNamespace(vivo=True, iniciando=False, in_progress=False, pending={}, question=None, sid=sid)
    hl._sessions["s"] = live
    hl.escolhas = lambda name: (None, None)
    resumed = []

    async def ensure(name, **kwargs):
        record = store.transfer_for_session(name)
        if record and record.phase == store.TransferPhase.RESTORING:
            resumed.append(name)
            live.vivo = True
        return live

    async def stop(name):
        live.vivo = False
    hl.ensure_running, hl.parar = ensure, stop
    hl.acordar = Mock(side_effect=AssertionError("não pode agendar nem drenar para provar restauração"))
    hl.send_prompt = AsyncMock(side_effect=AssertionError("não pode reenviar prompt"))
    adapter = lambda provider: hl if provider == CLAUDE_HEADLESS else cx
    monkeypatch.setattr("app.adapters.get_adapter", adapter)
    monkeypatch.setattr(api, "get_adapter", adapter)
    monkeypatch.setattr(api, "registry", reg)
    monkeypatch.setattr(api, "_invalidate_lists", Mock())
    monkeypatch.setattr(api, "session_life", lambda name: "k:source-key")
    monkeypatch.setattr(api.share_bridge, "set_life", Mock())
    monkeypatch.setattr(api.guest_users, "set_life", Mock())
    monkeypatch.setattr(api.settings, "auth_token", "test-secret")
    account = SimpleNamespace(id="chosen", home=tmp_path / ".codex-chosen")
    credential = f"codex:{account.home}"
    monkeypatch.setattr(store, "_resolve_target", lambda value: account if value == credential else
                        (_ for _ in ()).throw(store.TransferError("session_transfer_unknown_account")))
    monkeypatch.setattr(store, "_check_target", AsyncMock())
    thread_id = str(uuid.uuid4())
    rollout = tmp_path / f"rollout-test-{thread_id}.jsonl"
    failures = set()
    calls = []

    def fail(stage):
        calls.append(stage)
        if stage in failures:
            raise RuntimeError("conteúdo privado do erro não pode ser público")

    class Native:
        server_requests = {}
        async def request(self, method, params):
            fail(method)
            if method == "thread/loaded/list":
                return {"data": [thread_id]}
            if method == "config/read":
                return {"config": {"tool_output_token_limit": 5000}}
            if method == "thread/read":
                return {"thread": {"id": thread_id, "path": str(rollout), "cwd": str(tmp_path),
                        "model": "test-model", "reasoningEffort": "low", "status": {"type": "idle"}, "turns": []}}
            raise AssertionError(method)
        async def close(self):
            calls.append("close")

    async def prepare(account, cwd, context, model, effort, permission_mode, *, transfer_id, collaboration_mode):
        fail("inject")
        record = store.load_transfer(transfer_id)
        assert record.phase == store.TransferPhase.SOURCE_STOPPED
        assert not cs.list_all()
        raw = (json.dumps({"type": "session_meta", "payload": {"id": thread_id, "cwd": cwd}}) + "\n").encode()
        rollout.write_bytes(raw)
        boundary = store.ImportBoundary(thread_id, str(rollout), len(raw), (), hashlib.sha256(raw).hexdigest())
        target = {"thread_id": thread_id, "rollout_path": str(rollout), "codex_home": str(account.home),
                  "codex_account": account.id, "tool_output_token_limit": 5000, "cwd": cwd,
                  "permission_mode": permission_mode, "transfer_id": transfer_id,
                  "model": "test-model", "effort": "low"}
        store.save_transfer(replace(record, destination_meta=target, boundary=boundary))
        fail("persistence")
        return importer.PreparedCodexThread(thread_id, str(rollout), "test-model", "low", collaboration_mode, 5000, boundary)

    async def adopt(name, prepared, target, *, terminal):
        fail("resume")
        assert not terminal
        cs.write_prepared(name, target["transfer_id"], target)
        cx._adoptions = {name: (target["transfer_id"], Native(), prepared.mode)}
        fail("adoption")

    def attach(name, client, thread_id, **kwargs):
        cx._sessions[name] = {"client": client, "thread_id": thread_id}

    monkeypatch.setattr(importer, "prepare_import", prepare)
    cx.adopt_imported, cx.attach = adopt, attach
    body = {"credential_id": credential, "source_life": "k:source-key", "source_jsonl": str(source),
            "model": "test-model", "effort": "low"}

    async def run():
        return await store.transfer_claude_to_codex(reg, "s", **body)

    return SimpleNamespace(reg=reg, hl=hl, cx=cx, source=source, original=original, info=info,
                           meta=meta, body=body, run=run, live=live, failures=failures, calls=calls,
                           resumed=resumed, account=account, credential=credential, thread_id=thread_id)


def test_legacy_body_is_unchanged():
    body = api.AccountMoveBody(config_dir="/example/.claude")
    assert body.config_dir == "/example/.claude" and body.credential_id is None


@pytest.mark.parametrize("body", [{}, {"config_dir": "a", "credential_id": "codex:b"},
    {"config_dir": "a", "model": "x"}, {"credential_id": "codex:a", "source_life": "k:a"},
    {"credential_id": "claude:a", "source_life": "k:a", "source_jsonl": "/x"}])
def test_account_route_rejects_ambiguous_or_incomplete_target(body):
    with pytest.raises(ValidationError):
        api.AccountMoveBody(**body)


def test_blank_overrides_are_absence(scenario):
    body = api.AccountMoveBody(**{**scenario.body, "model": "  ", "effort": ""})
    assert body.model is None and body.effort is None


@pytest.mark.parametrize("field,value", [("source_life", "k:other"), ("source_jsonl", "/not/authorized.jsonl")])
def test_post_rejects_stale_source_without_import(scenario, field, value):
    response = TestClient(api.app).post("/api/sessions/s/conta", json={**scenario.body, field: value},
                                       headers={"Authorization": "Bearer test-secret"})
    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "session_transfer_source_changed"
    assert scenario.calls == [] and scenario.source.read_bytes() == scenario.original


async def test_preserves_identity_original_and_native_response(scenario):
    result = await scenario.run()
    assert result == {"ok": True, "provider": "codex", "conta": scenario.credential,
                      "model": "test-model", "effort": "low", "transfer_id": result["transfer_id"]}
    record = store.load_transfer(result["transfer_id"])
    assert record.phase == store.TransferPhase.COMPLETE
    assert cs.load("s")["key"] == scenario.meta["key"] == record.origin_meta["key"]
    assert cs.load("s")["thread_id"] == scenario.thread_id
    assert scenario.source.read_bytes() == scenario.original and not hs.exists("s")
    assert not scenario.resumed and not scenario.hl.send_prompt.called and not scenario.hl.acordar.called


@pytest.mark.parametrize("stage", ["inject", "persistence", "resume", "adoption", "thread/read", "thread/loaded/list"])
async def test_faults_restore_original_without_replay(scenario, stage):
    scenario.failures.add(stage)
    with pytest.raises(store.TransferError) as exc:
        await scenario.run()
    assert exc.value.code == "session_transfer_failed"
    assert scenario.resumed == ["s"]
    assert hs.load("s")["session_id"] == scenario.meta["session_id"]
    assert not cs.exists("s") and scenario.source.read_bytes() == scenario.original
    record = store.transfer_for_session("s")
    assert record.phase == store.TransferPhase.ROLLED_BACK
    assert not scenario.hl.send_prompt.called and not scenario.hl.acordar.called


@pytest.mark.parametrize("delivered", [False, True])
async def test_real_guard_refuses_pending_and_unconfirmed_delivery(scenario, delivered):
    entry = pqueue.PromptQueue("s").append("guardar entrada", delivered=delivered)
    with pytest.raises(store.TransferError, match="session_transfer_queue_pending"):
        await scenario.run()
    assert pqueue.PromptQueue("s").load()[0]["id"] == entry["id"]
    assert scenario.live.vivo and not scenario.calls


@pytest.mark.parametrize("field,value", [("in_progress", True), ("pending", {"approval": 1}),
                                         ("question", {"question": 1}), ("iniciando", True)])
async def test_real_guard_refuses_active_work(scenario, field, value):
    setattr(scenario.live, field, value)
    with pytest.raises(store.TransferError, match="session_transfer_source_busy"):
        await scenario.run()
    assert scenario.live.vivo and not scenario.calls


async def test_dead_quota_source_with_trusted_transcript_can_transfer(scenario):
    scenario.live.vivo = False
    result = await scenario.run()
    assert result["ok"]


@pytest.mark.parametrize("mode,base,wanted", [("manual", None, "Ask for approval"),
    ("acceptEdits", None, "Approve for me"), ("bypassPermissions", None, "Full Access"),
    ("plan", "manual", "Ask for approval"), ("plan", "bypassPermissions", "Full Access")])
def test_permission_and_plan_are_separate(mode, base, wanted):
    assert store._map_permission({"permission_mode": mode, "previous_non_plan": base}) == (
        wanted, "plan" if mode == "plan" else "default")


@pytest.mark.parametrize("mode", [None, "auto", "dontAsk", "unknown", "plan"])
def test_unknown_permission_never_falls_back_to_full_access(mode):
    with pytest.raises(store.TransferError):
        store._map_permission({"permission_mode": mode})


async def test_restore_failed_is_durable_and_reload_recovers(scenario, monkeypatch):
    scenario.failures.add("inject")
    restore = scenario.reg.restore_transfer_source
    monkeypatch.setattr(scenario.reg, "restore_transfer_source", AsyncMock(side_effect=RuntimeError("private")))
    with pytest.raises(store.TransferError, match="session_transfer_restore_failed"):
        await scenario.run()
    record = store.transfer_for_session("s")
    assert record.phase == store.TransferPhase.RESTORE_FAILED and store.transfer_active("s")
    monkeypatch.setattr(scenario.reg, "restore_transfer_source", restore)
    result = await api.recarregar_sessao("s")
    assert result["provider"] == "claude"
    assert store.load_transfer(record.id).phase == store.TransferPhase.ROLLED_BACK
    assert scenario.resumed == ["s"]


@pytest.mark.parametrize("phase", ["source_stopped", "inject"])
async def test_http_cancellation_waits_until_operation_converges(scenario, monkeypatch, phase):
    entered, release = asyncio.Event(), asyncio.Event()
    target = scenario.reg if phase == "source_stopped" else importer
    attribute = "stop_transfer_source" if phase == "source_stopped" else "prepare_import"
    original = getattr(target, attribute)

    async def pause(*args, **kwargs):
        result = await original(*args, **kwargs)
        entered.set()
        await release.wait()
        return result

    monkeypatch.setattr(target, attribute, pause)
    request = asyncio.create_task(api.trocar_conta("s", api.AccountMoveBody(**scenario.body)))
    await entered.wait()
    request.cancel()
    await asyncio.sleep(0)
    assert not request.done() and "s" in api.share_bridge.changing_mode
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await request
    record = store.transfer_for_thread(str(scenario.account.home), scenario.thread_id)
    assert record.phase == store.TransferPhase.COMPLETE
    assert "s" not in api.share_bridge.changing_mode


async def test_duplicate_operation_is_rejected_without_waiting(scenario):
    with store.session_operation("s"):
        with pytest.raises(store.TransferError, match="session_transfer_busy"):
            await scenario.run()
    assert not scenario.calls


async def test_terminal_send_lock_is_nonblocking(scenario):
    from app.terminal_input import _send_lock
    lock = _send_lock("s")
    lock.acquire()
    try:
        with pytest.raises(store.TransferError, match="session_transfer_busy"):
            await asyncio.wait_for(scenario.run(), 1)
    finally:
        lock.release()


async def test_transfer_rows_keep_owner_without_physical_poll(scenario, monkeypatch):
    row = SessionInfo(name="s", transfer_id=str(uuid.uuid4()), transfer_phase="restore_failed")
    monkeypatch.setattr(registry_module.guest_users, "has_claims", lambda: True)
    monkeypatch.setattr(registry_module.guest_users, "owner_name", lambda name: "dono")
    monkeypatch.setattr(registry_module.tmux, "capture_pane", Mock(side_effect=AssertionError("não pode ler estado projetado")))
    result = await scenario.reg.list_with_state([row])
    # `shared` é marcado pela lista do Rust, dona dos convites.
    assert result == [row] and row.owner == "dono"


def test_prepared_runtime_is_not_discoverable(scenario):
    transfer_id = str(uuid.uuid4())
    meta = {"name": "s", "transfer_id": transfer_id, "thread_id": scenario.thread_id,
            "key": "source-key", "codex_home": str(scenario.account.home)}
    cs.write_prepared("s", transfer_id, meta)
    assert cs.load("s") is None and cs.list_all() == []
    with pytest.raises(ValueError):
        cs.write_prepared("s", transfer_id, {**meta, "key": "other"})
    assert cs.load_prepared("s", transfer_id) == meta


@pytest.mark.parametrize("provider,engine,tracked", [("codex", None, True), ("claude", "engine", True),
                                                     ("claude", None, False)])
async def test_wrong_provider_engine_or_untracked_source_is_refused(scenario, provider, engine, tracked):
    scenario.info.provider, scenario.info.engine, scenario.info.tracked = provider, engine, tracked
    with pytest.raises(store.TransferError, match="session_transfer_source_changed"):
        await scenario.run()
    assert scenario.live.vivo and not scenario.calls


async def test_rejection_after_preparing_does_not_reopen_source(scenario, monkeypatch):
    check = store._check_source_idle
    count = 0

    async def starts_work(*args, **kwargs):
        nonlocal count
        count += 1
        if count == 2:
            scenario.live.in_progress = True
        await check(*args, **kwargs)

    monkeypatch.setattr(store, "_check_source_idle", starts_work)
    with pytest.raises(store.TransferError, match="session_transfer_source_busy"):
        await scenario.run()
    record = store.transfer_for_session("s")
    assert record.phase == store.TransferPhase.REJECTED
    assert scenario.live.vivo and not scenario.resumed


async def test_snapshot_failure_restores_source(scenario, monkeypatch):
    monkeypatch.setattr(store, "capture_snapshot", Mock(side_effect=OSError("private")))
    with pytest.raises(store.TransferError, match="session_transfer_failed"):
        await scenario.run()
    assert scenario.resumed == ["s"] and scenario.source.read_bytes() == scenario.original
    assert store.transfer_for_session("s").phase == store.TransferPhase.ROLLED_BACK


async def test_complete_commit_failure_rolls_back_published_codex(scenario, monkeypatch):
    save = store.save_transfer

    def fail_commit(record):
        if record.phase == store.TransferPhase.COMPLETE:
            raise OSError("disk")
        return save(record)

    monkeypatch.setattr(store, "save_transfer", fail_commit)
    with pytest.raises(store.TransferError):
        await scenario.run()
    assert not cs.exists("s") and hs.exists("s")
    assert store.transfer_for_session("s").phase == store.TransferPhase.ROLLED_BACK
    assert scenario.resumed == ["s"]


async def test_source_written_after_stop_cannot_publish(scenario, monkeypatch):
    adopt = scenario.cx.adopt_imported

    async def changed(*args, **kwargs):
        await adopt(*args, **kwargs)
        with scenario.source.open("ab") as source:
            source.write(b'{}\n')

    monkeypatch.setattr(scenario.cx, "adopt_imported", changed)
    with pytest.raises(store.TransferError, match="session_transfer_restore_failed"):
        await scenario.run()
    assert not cs.exists("s")
    assert store.transfer_for_session("s").phase == store.TransferPhase.RESTORE_FAILED


async def test_terminal_native_state_is_checked_even_when_list_says_idle(scenario, monkeypatch):
    scenario.info.headless = False
    scenario.live.vivo = False
    pane = {"pid": 901, "cwd": scenario.info.cwd}
    monkeypatch.setattr(scenario.reg, "_pane_of", lambda name: pane)
    monkeypatch.setattr(registry_module, "_pid_do_agente", lambda pid: 902)
    monkeypatch.setattr(registry_module, "provider_of_pane", lambda *args: "claude")
    monkeypatch.setattr(registry_module.procinfo, "_proc_children_map", lambda **kwargs: {})
    monkeypatch.setattr(registry_module.procinfo, "_engine_of", lambda pid: None)
    native_dir = Path(scenario.meta["config_dir"]) / "sessions"
    native_dir.mkdir()
    native = native_dir / "902.json"
    native.write_text(json.dumps({"sessionId": scenario.meta["session_id"], "pid": 902, "status": "busy"}))
    meta = {**scenario.meta, "jsonl": str(scenario.source), "headless": False, "pane_pid": 901}
    with pytest.raises(store.TransferError, match="session_transfer_source_busy"):
        await store._check_source_idle(scenario.reg, "s", meta)
    native.write_text(json.dumps({"sessionId": "changed", "pid": 902, "status": "idle"}))
    with pytest.raises(store.TransferError, match="session_transfer_source_changed"):
        await store._check_source_idle(scenario.reg, "s", meta)


async def test_ingress_and_controls_are_temporarily_unavailable(scenario, monkeypatch):
    record = store.TransferRecord(str(uuid.uuid4()), "s", "k:source-key", store.TransferPhase.PREPARING,
                                  None, {**scenario.meta, "jsonl": str(scenario.source)}, None, None, None)
    store.save_transfer(record)
    monkeypatch.setattr(api, "_send_native_available", Mock(side_effect=AssertionError("não pode enviar")))
    assert api._send_one("s", "guardar")["error"]["code"] == "session_transfer_busy"
    assert (await api._enviar("s", "guardar"))["error"]["code"] == "session_transfer_busy"
    assert api._enviar_nativo("s", "[de: peer] guardar") is None
    assert api.terminal.send_prompt("s", "guardar") == "deferred"
    assert not api.plugin_bridge.entregar("s", "guardar")
    assert pqueue.PromptQueue("s").load() == []
    client = TestClient(api.app)
    for method, path, body in [("delete", "/api/sessions/s", None),
                               ("post", "/api/sessions/s/keys", {"key": "enter"})]:
        response = client.request(method, path, json=body, headers={"Authorization": "Bearer test-secret"})
        assert response.status_code == 409
        assert response.json()["detail"]["code"] == "session_transfer_busy"


def test_recycled_pid_never_receives_signal(scenario, monkeypatch):
    from app import procinfo
    record = store.TransferRecord(str(uuid.uuid4()), "s", "k:source-key", store.TransferPhase.SOURCE_STOPPED,
                                  None, scenario.meta, None, None, None)
    store.prepare_runtime(record)
    store._write_json(store._runtime_path(record), {"original": {}, "processes": {}, "import_pid": 9,
        "import_processes": {"9": {"started": 1, "command": "old"}}})
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: True)
    monkeypatch.setattr(store, "_process_identity", lambda pid: {"started": 2, "command": "other"})
    kill = Mock(side_effect=AssertionError("não pode matar PID reciclado"))
    monkeypatch.setattr(store.os, "kill", kill)
    with pytest.raises(store.TransferError, match="session_transfer_process_identity_changed"):
        asyncio.run(store.stop_import_process(record))
    assert not kill.called


async def test_terminal_adoption_loads_same_thread_before_publishing(scenario, monkeypatch):
    from app.adapters.codex import adapter as adapter_module
    result = await scenario.run()
    record = store.load_transfer(result["transfer_id"])
    cs.delete("s")
    scenario.cx._sessions.clear()
    meta = {**record.destination_meta, "headless": False}
    prepared = importer.PreparedCodexThread(scenario.thread_id, record.boundary.rollout_path,
        "test-model", "low", "default", 5000, record.boundary)
    spawned = False
    requests = []

    class Client:
        endpoint = None
        server_requests = {}
        async def connect(self, endpoint):
            self.endpoint = endpoint
        async def close(self):
            pass
        async def request(self, method, params):
            requests.append(method)
            thread = {"id": scenario.thread_id, "path": record.boundary.rollout_path,
                      "cwd": scenario.info.cwd, "model": "test-model", "reasoningEffort": "low",
                      "turns": [], "status": {"type": "idle"}}
            if method in {"initialize", "thread/settings/update"}:
                return {}
            if method == "thread/loaded/list":
                return {"data": [scenario.thread_id]}
            if method == "thread/read":
                return {"thread": thread}
            if method == "config/read":
                return {"config": {"tool_output_token_limit": 5000}}
            if method == "thread/resume":
                assert "cwd" not in params
                return {"thread": thread, "approvalPolicy": "on-request", "sandbox": {"type": "readOnly"}}
            raise AssertionError(method)

    def spawn(name, cwd, command, **kwargs):
        nonlocal spawned
        spawned = True
        assert "--transfer-id" in command and record.id in command
        assert kwargs["env"]["CP_SESSION_KEY"] == record.origin_meta["key"]
        assert cs.load(name) is None
        cs.update_transfer_runtime(name, record.id, endpoint="ws://127.0.0.1:9999", app_pid=12, tui_pid=13)
        return True

    monkeypatch.setattr(adapter_module, "AppServerClient", Client)
    monkeypatch.setattr(adapter_module.codex_contas, "resolve_account", lambda name: scenario.account)
    monkeypatch.setattr(adapter_module, "pid_vivo", lambda pid: True)
    monkeypatch.setattr(registry_module, "_exigir_lancador_codex", Mock())
    monkeypatch.setattr(store, "_processes", lambda pid: {str(pid): {"started": 1, "command": "test"}})
    monkeypatch.setattr(registry_module.tmux, "has_session", lambda name: spawned)
    monkeypatch.setattr(registry_module.tmux, "pane_pid", lambda name: 11)
    monkeypatch.setattr(registry_module.tmux, "new_session", spawn)
    await CodexAdapter.adopt_imported(scenario.cx, "s", prepared, meta, terminal=True)
    assert cs.load("s") is None and cs.list_all() == []
    assert "thread/loaded/list" in requests and "thread/resume" in requests
    assert "turn/start" not in requests and "thread/start" not in requests
    await scenario.cx.publish_imported(record)
    assert cs.load("s")["thread_id"] == scenario.thread_id
    assert cs.load("s")["key"] == record.origin_meta["key"]


def test_crash_without_final_child_snapshot_never_claims_cleanup(scenario, monkeypatch):
    from app import procinfo
    record = store.TransferRecord(str(uuid.uuid4()), "s", "k:source-key", store.TransferPhase.SOURCE_STOPPED,
                                  None, scenario.meta, None, None, None)
    store.prepare_runtime(record)
    store._write_json(store._runtime_path(record), {"original": {}, "processes": {}, "import_pid": 9,
        "import_processes": {"9": {"started": 1, "command": "owned"}}})
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: False)
    kill = Mock(side_effect=AssertionError("não há prova de filhos por nome"))
    monkeypatch.setattr(store.os, "kill", kill)
    with pytest.raises(store.TransferError, match="session_transfer_import_cleanup_unconfirmed"):
        asyncio.run(store.stop_import_process(record))
    assert not kill.called


@pytest.mark.parametrize("state,pct,code", [("expirada", 0, "session_transfer_login_required"),
    ("lida", 99, "session_transfer_account_full"), ("lida", 10, "session_transfer_invalid_model_choice")])
async def test_target_uses_codex_quota_and_validates_model_before_stop(tmp_path, monkeypatch, state, pct, code):
    from app import cotas, codex_models, codex_appserver
    account = SimpleNamespace(home=tmp_path / ".codex-target")
    monkeypatch.setattr(codex_appserver, "versao", lambda: importer.SUPPORTED_VERSION)
    reads = []
    monkeypatch.setattr(cotas, "_ler_codex", lambda home: reads.append(home) or
                        (state, [SimpleNamespace(pct=pct)], None))
    monkeypatch.setattr(cotas, "cotas_claude", Mock(side_effect=AssertionError("não é cota Claude")))
    monkeypatch.setattr(codex_models, "checar_escolha", Mock(side_effect=ValueError("private")))
    with pytest.raises(store.TransferError) as error:
        await store._check_target(account, "invalid", "low")
    assert error.value.code == code and reads == [account.home]


def test_credential_path_is_only_matched_to_registered_account(tmp_path, monkeypatch):
    from app import codex_contas
    listed = SimpleNamespace(id="known", home=tmp_path / ".codex-known")
    monkeypatch.setattr(codex_contas, "list_accounts", lambda: [listed])
    resolve = Mock(side_effect=AssertionError("caminho arbitrário não pode resolver conta"))
    monkeypatch.setattr(codex_contas, "resolve_account", resolve)
    with pytest.raises(store.TransferError, match="session_transfer_unknown_account"):
        store._resolve_target(f"codex:{tmp_path / 'arbitrary'}")
    assert not resolve.called


@pytest.fixture
def terminal_ingress(scenario, monkeypatch):
    """Transportes simulados; decisão de envio e fila continuam nas funções de produção."""
    monkeypatch.setattr(api, "_headless", lambda name: False)
    monkeypatch.setattr(api, "_pane_info", lambda name: ("claude", None))
    monkeypatch.setattr(api, "_enviar_nativo", lambda *args: None)
    monkeypatch.setattr(api.plugin_bridge, "aguardando", lambda name: False)
    monkeypatch.setattr(api, "_recusa_orq", Mock())
    monkeypatch.setattr(api, "_session_exists", lambda name: True)
    monkeypatch.setattr(api, "_agendar_confirmacao", Mock())
    monkeypatch.setattr(api, "_drain_session", Mock())
    monkeypatch.setattr(api.tmux, "list_panes_active", lambda: [])
    monkeypatch.setattr(scenario.reg, "resolve_tracked", lambda *args: (str(scenario.source), True))
    monkeypatch.setattr(api.terminal, "interrupt", Mock())
    return scenario


@pytest.mark.parametrize("delivery", ["sent", "deferred"])
async def test_enviar_holds_destination_until_terminal_receipt_is_persisted(terminal_ingress, monkeypatch, delivery):
    import threading
    scenario = terminal_ingress
    entered, release = threading.Event(), threading.Event()
    append = pqueue.PromptQueue.append

    def transport(name, *args, **kwargs):
        with api.terminal_input._send_lock(name):
            return delivery

    def paused_append(queue, *args, **kwargs):
        entered.set()
        assert release.wait(5)
        return append(queue, *args, **kwargs)

    monkeypatch.setattr(api.terminal, "send_prompt", transport)
    monkeypatch.setattr(pqueue.PromptQueue, "append", paused_append)
    request = asyncio.create_task(api._enviar("s", "recado"))
    try:
        assert await asyncio.to_thread(entered.wait, 5)
        # A entrega já soltou o mutex terminal; falta persistir o recibo.
        terminal_lock = api.terminal_input._send_lock("s")
        assert terminal_lock.acquire(blocking=False)
        terminal_lock.release()
        assert pqueue.PromptQueue("s").load() == []
        with pytest.raises(store.TransferError, match="session_transfer_busy"):
            await scenario.run()
        assert scenario.live.vivo and not scenario.calls
    finally:
        release.set()
        response = await request
    assert response["ok"] and response["delivered"] is (delivery == "sent")
    rows = pqueue.PromptQueue("s").load()
    assert len(rows) == 1 and rows[0]["delivered"] is (delivery == "sent") and not rows[0].get("confirmed")
    with pytest.raises(store.TransferError, match="session_transfer_queue_pending"):
        await scenario.run()
    assert scenario.live.vivo


async def test_cancelled_common_send_keeps_participation_until_receipt(terminal_ingress, monkeypatch):
    import threading
    entered, release = threading.Event(), threading.Event()
    append = pqueue.PromptQueue.append
    monkeypatch.setattr(api.terminal, "send_prompt", lambda *args, **kwargs: "sent")

    def paused_append(queue, *args, **kwargs):
        entered.set()
        assert release.wait(5)
        return append(queue, *args, **kwargs)

    monkeypatch.setattr(pqueue.PromptQueue, "append", paused_append)
    request = asyncio.create_task(api._enviar("s", "recado cancelado"))
    try:
        assert await asyncio.to_thread(entered.wait, 5)
        request.cancel()
        await asyncio.sleep(0)
        assert not request.done()
        with pytest.raises(store.TransferError, match="session_transfer_busy"):
            await terminal_ingress.run()
    finally:
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await request
    assert len(pqueue.PromptQueue("s").load()) == 1
    with store.session_operation("s"):
        pass


async def test_slow_query_does_not_exclude_input_or_interrupt(terminal_ingress, monkeypatch):
    import threading
    from httpx2 import ASGITransport, AsyncClient
    entered, release = threading.Event(), threading.Event()

    def slow(*args, **kwargs):
        entered.set()
        assert release.wait(5)
        return "resultado"

    monkeypatch.setattr(api, "file_diff", slow)
    monkeypatch.setattr(api, "_session_cwd", lambda name: terminal_ingress.info.cwd)
    monkeypatch.setattr(api.terminal, "send_prompt", lambda *args, **kwargs: "deferred")
    headers = {"Authorization": "Bearer test-secret"}
    async with AsyncClient(transport=ASGITransport(app=api.app), base_url="http://test", headers=headers) as client:
        pending = asyncio.create_task(client.post("/api/sessions/s/git/diff", json={"path": "example.py"}))
        try:
            assert await asyncio.to_thread(entered.wait, 5)
            # Consulta pendente também não prende a operação exclusiva de troca.
            with store.session_operation("s"):
                pass
            sent = await client.post("/api/sessions/s/input", json={"text": "mensagem"})
            interrupted = await client.post("/api/sessions/s/interrupt")
            assert sent.status_code == 200 and interrupted.status_code == 200
            assert api.terminal.interrupt.called and not pending.done()
        finally:
            release.set()
            response = await pending
        assert response.status_code == 200


async def test_two_normal_http_inputs_share_the_ingress_gate(terminal_ingress, monkeypatch):
    import threading
    from httpx2 import ASGITransport, AsyncClient
    first, both, release = threading.Event(), threading.Event(), threading.Event()
    arrivals, lock = [], threading.Lock()
    append = pqueue.PromptQueue.append
    monkeypatch.setattr(api.terminal, "send_prompt", lambda *args, **kwargs: "deferred")

    def paused_append(queue, text, **kwargs):
        with lock:
            arrivals.append(text)
            first.set()
            if len(arrivals) == 2:
                both.set()
        assert release.wait(5)
        return append(queue, text, **kwargs)

    monkeypatch.setattr(pqueue.PromptQueue, "append", paused_append)
    headers = {"Authorization": "Bearer test-secret"}
    async with AsyncClient(transport=ASGITransport(app=api.app), base_url="http://test", headers=headers) as client:
        requests = [asyncio.create_task(client.post("/api/sessions/s/input", json={"text": "primeira"}))]
        try:
            assert await asyncio.to_thread(first.wait, 5)
            requests.append(asyncio.create_task(client.post("/api/sessions/s/input", json={"text": "segunda"})))
            assert await asyncio.to_thread(both.wait, 5)
            with pytest.raises(store.TransferError, match="session_transfer_busy"):
                await terminal_ingress.run()
        finally:
            release.set()
            responses = await asyncio.gather(*requests)
        assert all(response.status_code == 200 for response in responses)
    assert {row["text"] for row in pqueue.PromptQueue("s").load()} == {"primeira", "segunda"}


async def test_transfer_still_refuses_queries_input_and_interrupt(terminal_ingress, monkeypatch):
    from httpx2 import ASGITransport, AsyncClient
    scenario = terminal_ingress
    record = store.TransferRecord(str(uuid.uuid4()), "s", "k:source-key", store.TransferPhase.PREPARING,
                                  None, {**scenario.meta, "jsonl": str(scenario.source)}, None, None, None)
    store.save_transfer(record)
    never = Mock(side_effect=AssertionError("handler não deve executar durante a transferência"))
    monkeypatch.setattr(api, "file_diff", never)
    monkeypatch.setattr(api.terminal, "send_prompt", never)
    monkeypatch.setattr(api.terminal, "interrupt", never)
    headers = {"Authorization": "Bearer test-secret"}
    async with AsyncClient(transport=ASGITransport(app=api.app), base_url="http://test", headers=headers) as client:
        requests = [client.post("/api/sessions/s/git/diff", json={"path": "example.py"}),
                    client.post("/api/sessions/s/input", json={"text": "mensagem"}),
                    client.post("/api/sessions/s/interrupt")]
        responses = await asyncio.gather(*requests)
    assert all(response.status_code == 409 for response in responses)
    assert all(response.json()["detail"]["code"] == "session_transfer_busy" for response in responses)
    assert not never.called and pqueue.PromptQueue("s").load() == []


async def test_delivery_wait_rechecks_durable_transfer_before_enqueue(scenario):
    lock = scenario.hl.delivery_lock("s")
    await lock.acquire()
    request = asyncio.create_task(api._send_one_headless("s", "mensagem"))
    try:
        await asyncio.sleep(0)
        assert not request.done()
        record = store.TransferRecord(str(uuid.uuid4()), "s", "k:source-key", store.TransferPhase.PREPARING,
                                      None, {**scenario.meta, "jsonl": str(scenario.source)}, None, None, None)
        store.save_transfer(record)
    finally:
        lock.release()
    response = await request
    assert not response["ok"] and response["error"]["code"] == "session_transfer_busy"
    assert pqueue.PromptQueue("s").load() == [] and not scenario.hl.send_prompt.called


def cleanup_record(scenario):
    record = store.TransferRecord(str(uuid.uuid4()), "s", "k:source-key", store.TransferPhase.SOURCE_STOPPED,
                                  None, scenario.meta, None, None, None)
    store.prepare_runtime(record)
    store._write_json(store._runtime_path(record), {"original": {}, "processes": {}, "import_pid": 9,
        "import_tree_final": True, "import_processes": {
            "9": {"started": 1, "command": "root"}, "10": {"started": 1, "command": "child"}}})
    return record


async def test_normal_import_cleanup_waits_for_captured_child_after_root_exits(scenario, monkeypatch):
    from app import procinfo
    record = cleanup_record(scenario)
    child_polls = 0

    def alive(pid):
        nonlocal child_polls
        if pid == 9:
            return False
        assert pid == 10
        child_polls += 1
        return child_polls < 3

    monkeypatch.setattr(procinfo, "pid_vivo", alive)
    monkeypatch.setattr(store, "_process_identity", lambda pid: {"started": 1, "command": "child"})
    kill = Mock(side_effect=AssertionError("fechamento normal só espera a árvore capturada"))
    monkeypatch.setattr(store.os, "kill", kill)
    await store.wait_import_exit(record)
    assert child_polls == 3 and store._runtime(record)["import_stopped"] is True
    assert not kill.called


@pytest.mark.parametrize("identity", [None, {"started": 1, "command": "child"},
                                      {"started": 1, "command": "different-command"}])
async def test_normal_cleanup_deadline_does_not_confirm_live_or_unknown_child(scenario, monkeypatch, identity):
    from app import procinfo
    record = cleanup_record(scenario)
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: pid == 10)
    monkeypatch.setattr(store, "_process_identity", lambda pid: identity)
    kill = Mock(side_effect=AssertionError("prazo esgotado não autoriza sinalizar o processo"))
    monkeypatch.setattr(store.os, "kill", kill)
    with pytest.raises(store.TransferError, match="session_transfer_import_not_stopped"):
        await store.wait_import_exit(record, timeout=0)
    assert not store._runtime(record).get("import_stopped") and not kill.called


async def test_normal_cleanup_leaves_reused_pid_untouched(scenario, monkeypatch):
    from app import procinfo
    record = cleanup_record(scenario)
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: pid == 10)
    monkeypatch.setattr(store, "_process_identity", lambda pid: {"started": 2, "command": "unrelated"})
    kill = Mock(side_effect=AssertionError("PID de outro nascimento não é o filho capturado"))
    monkeypatch.setattr(store.os, "kill", kill)
    await store.wait_import_exit(record, timeout=0)
    private = store._runtime(record)
    assert private["import_stopped"] is True
    assert private["import_processes"]["10"] == {"started": 1, "command": "child"}
    assert not kill.called


@pytest.mark.parametrize("previous", [None, "manual", "bypassPermissions"])
async def test_terminal_plan_uses_only_observed_base_after_backend_restart(scenario, monkeypatch, previous):
    from collections import OrderedDict
    from app import permission_mode
    hs.delete("s")
    scenario.info.headless = False
    account = Path(scenario.meta["config_dir"])
    (account / "settings.json").write_text(json.dumps({"permissions": {"defaultMode": "bypassPermissions"}}))
    monkeypatch.setattr(permission_mode, "_ultimos_nao_plan", OrderedDict())
    monkeypatch.setattr(permission_mode, "_modos_confirmados", {})
    if previous is not None:
        # O monitor grava pelo session-id (stem do jsonl), não pelo nome da sessão.
        permission_mode.observar_ou_confirmado(Path(scenario.info.jsonl).stem, previous)
    monkeypatch.setattr(scenario.reg, "_pane_of", lambda name: {"pane_id": "%test", "pid": None})
    monkeypatch.setattr(registry_module, "provider_of_pane", lambda pid: "claude")
    monkeypatch.setattr(registry_module, "_escolhas_status", lambda sid: (None, None))
    monkeypatch.setattr(permission_mode, "ler_modo", lambda name: "plan")
    stop = AsyncMock(side_effect=AssertionError("não estacionar com base desconhecida"))
    monkeypatch.setattr(scenario.reg, "stop_transfer_source", stop)
    public, private = scenario.reg.transfer_origin(scenario.info)
    assert public["previous_non_plan"] == private["original"]["previous_non_plan"] == previous
    assert permission_mode.modo_da_conta(str(account)) == "bypassPermissions"
    if previous is None:
        with pytest.raises(store.TransferError, match="session_transfer_invalid_permission_mode"):
            await scenario.run()
        assert not stop.called and not scenario.calls
    else:
        assert store._map_permission(public) == (
            "Ask for approval" if previous == "manual" else "Full Access", "plan")


async def test_terminal_plan_without_observed_base_uses_transcript(scenario, monkeypatch):
    from collections import OrderedDict
    from app import permission_mode
    hs.delete("s")
    scenario.info.headless = False
    monkeypatch.setattr(permission_mode, "_ultimos_nao_plan", OrderedDict())
    with open(scenario.info.jsonl, "a", encoding="utf-8") as f:
        f.write(json.dumps({"type": "user", "permissionMode": "bypassPermissions"}) + "\n")
        f.write(json.dumps({"type": "user", "permissionMode": "plan"}) + "\n")
    monkeypatch.setattr(scenario.reg, "_pane_of", lambda name: {"pane_id": "%test", "pid": None})
    monkeypatch.setattr(registry_module, "provider_of_pane", lambda pid: "claude")
    monkeypatch.setattr(registry_module, "_escolhas_status", lambda sid: (None, None))
    monkeypatch.setattr(permission_mode, "ler_modo", lambda name: "plan")
    public, _ = scenario.reg.transfer_origin(scenario.info)
    assert public["previous_non_plan"] == "bypassPermissions"
    assert store._map_permission(public) == ("Full Access", "plan")


@pytest.mark.parametrize("code,reason", [
    ("session_transfer_context_budget_exceeded", "excede a capacidade"),
    ("session_transfer_model_capacity_unknown", "confirmar a capacidade"),
    ("session_transfer_model_media_unsupported", "não aceita as imagens"),
    ("session_transfer_codex_version_unsupported", "versão instalada"),
    ("session_transfer_login_required", "Entre na conta"),
    ("session_transfer_account_full", "sem cota"),
    ("session_transfer_invalid_model_choice", "catálogo"),
    ("session_transfer_queue_pending", "mensagens na fila"),
    ("session_transfer_source_busy", "permissão pendente"),
])
def test_account_api_keeps_specific_safe_transfer_reasons(scenario, monkeypatch, code, reason):
    failure = store.TransferError(code, params={"model": "selected-model", "estimated": 123, "limit": 100})
    monkeypatch.setattr(store, "transfer_claude_to_codex", AsyncMock(side_effect=failure))
    client = TestClient(api.app)
    response = client.post("/api/sessions/s/conta", headers={"Authorization": "Bearer test-secret"},
                           json=scenario.body)
    assert response.status_code == 409
    detail = response.json()["detail"]
    assert detail["code"] == code and reason in detail["msg"]
    assert detail["params"] == {"model": "selected-model", "estimated": 123, "limit": 100}
    assert str(scenario.source) not in response.text and "test-secret" not in response.text


def test_guarded_request_can_upgrade_to_its_own_operation_but_not_past_others():
    from app.conversation_transfer import TransferError, session_ingress, session_operation
    with session_ingress("upgrade"):
        with session_operation("upgrade"):
            pass
        with session_ingress("upgrade"):
            pass
    import threading
    entered, release = threading.Event(), threading.Event()

    def other_request():
        with session_ingress("upgrade"):
            entered.set()
            release.wait(5)

    t = threading.Thread(target=other_request)
    t.start()
    entered.wait(5)
    try:
        with session_ingress("upgrade"):
            with pytest.raises(TransferError):
                with session_operation("upgrade"):
                    pass
    finally:
        release.set()
        t.join(5)
