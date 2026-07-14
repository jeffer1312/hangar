"""Task 5: lifecycle da sessao Codex no registry (nao-tmux, sidecar duravel + resume lazy).

Mocka o AppServerClient (NAO spawna o codex real). Cobre: create_codex (durable sidecar +
attach), list() incluindo Codex + Claude, nao-regressao do caminho Claude no create(), kill de
Codex (fecha client + apaga sidecar), e ensure_running pos-restart (dict vazio -> thread/resume)."""
import asyncio
import json

import pytest
from unittest.mock import patch

from app import registry
from app.registry import SessionRegistry
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex.adapter import CodexAdapter


@pytest.fixture(autouse=True)
def _isolate(tmp_path):
    # Cache de classe compartilhado -> zera entre testes. Sidecars redirecionados pra tmp.
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()
    sdir = tmp_path / "codex-sessions"
    with patch.object(codex_sessions, "_dir", lambda: sdir):
        yield
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()


class _FakeClient:
    """Duck-type de AppServerClient: grava requests, responde thread/start e thread/resume."""

    def __init__(self, *_args, **_kwargs):
        self.requests: list[tuple[str, dict]] = []
        self.started = False
        self.closed = False
        self._thread_id = "019f5c00-5d7d-7dd2-b2cb-085ca6d76251"
        self._path = "/home/u/.codex/sessions/2026/07/13/rollout-x.jsonl"

    async def start(self):
        self.started = True

    async def request(self, method, params, timeout=30.0):
        self.requests.append((method, params))
        if method in ("thread/start", "thread/resume"):
            return {"thread": {"id": self._thread_id, "path": self._path}}
        return {}

    def terminate(self):
        self.closed = True

    async def close(self):
        self.closed = True


# --- Teste 1: create_codex grava sidecar duravel + attach -----------------------------------

async def test_create_codex_writes_sidecar_and_returns_provider(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    fake = _FakeClient()
    adapter = CodexAdapter()
    with patch.object(registry, "AppServerClient", lambda *a, **k: fake), \
         patch("app.adapters.get_adapter", return_value=adapter), \
         patch.object(registry.tmux, "has_session", return_value=False):
        info = await reg.create_codex("mysess", "/tmp/proj")
    assert info.provider == "codex"
    assert info.jsonl == fake._path
    # sidecar duravel gravado com thread_id + rollout_path + cwd
    saved = codex_sessions.load("mysess")
    assert saved["thread_id"] == fake._thread_id
    assert saved["rollout_path"] == fake._path
    assert saved["cwd"] == "/tmp/proj"
    # client vivo anexado no adapter (memoria efemera)
    assert "mysess" in adapter._sessions
    # thread/start foi chamado com sandbox workspace-write (Codex pode editar arquivos)
    methods = [m for m, _ in fake.requests]
    assert "initialize" in methods and "thread/start" in methods
    start_params = next(p for m, p in fake.requests if m == "thread/start")
    assert start_params["sandbox"] == "workspace-write"


# --- Teste 2: list() inclui Codex (sidecar) E Claude (tmux) ---------------------------------

def test_list_includes_codex_sidecar_and_tmux(tmp_path):
    codex_sessions.save("cx", "tid-1", "/home/u/.codex/sessions/rollout-a.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    tmux_panes = [{"name": "claudesess", "cwd": "/tmp/c", "pid": 111}]
    with patch.object(registry.tmux, "list_panes_active", return_value=tmux_panes), \
         patch.object(registry, "_proc_children_map", return_value={}), \
         patch.object(SessionRegistry, "resolve_tracked", return_value=("/x/claude.jsonl", True)), \
         patch.object(SessionRegistry, "_repl_sid", return_value=None):
        out = reg.list()
    by_name = {s.name: s for s in out}
    assert by_name["claudesess"].provider == "claude"
    cx = by_name["cx"]
    assert cx.provider == "codex"
    assert cx.jsonl == "/home/u/.codex/sessions/rollout-a.jsonl"
    assert cx.tracked is True


# --- Teste 3: create(provider="claude") = nao-regressao (tmux, sem sidecar) -----------------

def test_create_claude_still_uses_tmux_no_sidecar(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        info = reg.create("claudesess", "/tmp/proj")
    assert info.provider == "claude"
    new_sess.assert_called_once()  # caminho tmux intacto
    # nenhum sidecar Codex gravado
    assert codex_sessions.load("claudesess") is None


# --- Teste 4: kill de Codex fecha o client (mock) e apaga o sidecar -------------------------

def test_kill_codex_closes_client_and_removes_sidecar(tmp_path):
    codex_sessions.save("cx", "tid-1", "/home/u/.codex/rollout-a.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    fake = _FakeClient()
    adapter = CodexAdapter()
    adapter.attach("cx", fake, "tid-1")
    with patch("app.adapters.get_adapter", return_value=adapter), \
         patch.object(registry.tmux, "kill_session") as kill_tmux:
        reg.kill("cx")
    assert fake.closed is True                      # client vivo terminado
    assert "cx" not in adapter._sessions            # esquecido da memoria
    assert codex_sessions.load("cx") is None        # sidecar duravel apagado
    kill_tmux.assert_not_called()                    # NAO toca tmux numa sessao Codex


# --- Colisao de nome cross-provider (review Important #1) ------------------------------------

def test_create_claude_rejects_existing_codex_name(tmp_path):
    codex_sessions.save("dup", "tid-1", "/home/u/.codex/rollout.jsonl", "/tmp/a")
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as new_sess:
        with pytest.raises(ValueError):
            reg.create("dup", "/tmp/proj")
    new_sess.assert_not_called()  # nao chegou a spawnar pane tmux orfao


async def test_create_codex_rejects_existing_tmux_name(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    adapter = CodexAdapter()
    with patch.object(registry.tmux, "has_session", return_value=True), \
         patch.object(registry, "AppServerClient", lambda *a, **k: _FakeClient()), \
         patch("app.adapters.get_adapter", return_value=adapter):
        with pytest.raises(ValueError):
            await reg.create_codex("dup", "/tmp/proj")


# --- Orfao de processo se save() falhar (review Important #2) --------------------------------

async def test_create_codex_closes_client_when_save_fails(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    fake = _FakeClient()
    adapter = CodexAdapter()
    with patch.object(registry, "AppServerClient", lambda *a, **k: fake), \
         patch("app.adapters.get_adapter", return_value=adapter), \
         patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(codex_sessions, "save", side_effect=OSError("disco cheio")):
        with pytest.raises(OSError):
            await reg.create_codex("mysess", "/tmp/proj")
    assert fake.closed is True                       # client fechado, sem orfao
    assert codex_sessions.load("mysess") is None     # nenhum sidecar orfao
    assert "mysess" not in adapter._sessions         # nao anexado


# --- Teste 5: ensure_running pos-restart reabre client e retoma pelo thread_id --------------

async def test_ensure_running_resumes_by_thread_id(tmp_path):
    # Simula pos-restart: sidecar no disco, dict de clients vazio.
    codex_sessions.save("cx", "tid-42", "/home/u/.codex/rollout-a.jsonl", "/tmp/a")
    adapter = CodexAdapter()
    assert "cx" not in adapter._sessions
    fake = _FakeClient()
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: fake):
        client = await adapter.ensure_running("cx")
    assert client is fake
    assert fake.started is True
    methods = [m for m, _ in fake.requests]
    assert "initialize" in methods
    # RETOMA o thread existente via thread/resume passando o threadId do sidecar
    assert "thread/resume" in methods
    resume_params = next(p for m, p in fake.requests if m == "thread/resume")
    assert resume_params["threadId"] == "tid-42"
    # anexado na memoria pra proximas chamadas
    assert "cx" in adapter._sessions


async def test_ensure_running_reuses_live_client(tmp_path):
    adapter = CodexAdapter()
    fake = _FakeClient()
    adapter.attach("cx", fake, "tid-1")
    client = await adapter.ensure_running("cx")
    assert client is fake
    assert fake.started is False  # nao reabriu nada


# --- IMPORTANT 1: lock por-nome evita spawn duplicado em ensure_running concorrente ----------

async def test_ensure_running_concurrent_calls_spawn_once(tmp_path):
    # 2 chamadores concorrentes pro mesmo nome, sem client vivo (pos-restart): sem lock, ambos
    # passavam pelo `sess is None`, ambos spawnavam+resumiam, e o 2o attach() sobrescrevia o 1o
    # AppServerClient no dict -- o 1o (subprocess + reader task) ficava orfao, nunca fechado.
    codex_sessions.save("cx-race", "tid-race", "/home/u/.codex/rollout-race.jsonl", "/tmp/race")
    adapter = CodexAdapter()
    starts = 0

    class _SlowFakeClient(_FakeClient):
        async def start(self):
            nonlocal starts
            starts += 1
            await asyncio.sleep(0)  # forca o interleaving entre as 2 corrotinas
            self.started = True

    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: _SlowFakeClient()):
        c1, c2 = await asyncio.gather(
            adapter.ensure_running("cx-race"), adapter.ensure_running("cx-race"),
        )
    assert starts == 1          # client.start() chamado 1x so
    assert c1 is c2             # os dois chamadores recebem o MESMO client
    assert len(adapter._sessions) == 1


async def test_ensure_running_double_check_skips_spawn_if_attached_meanwhile(tmp_path):
    # Versao deterministica (sem depender de timing real de interleaving): segura o lock na mao,
    # dispara ensure_running numa task (que bloqueia em `async with lock`), anexa a sessao como se
    # OUTRO chamador tivesse vencido a corrida, e so entao libera o lock -- o double-check tem que
    # ver a sessao ja anexada e NAO spawnar de novo.
    codex_sessions.save("cx-dc", "tid-dc", "/home/u/.codex/rollout-dc.jsonl", "/tmp/dc")
    adapter = CodexAdapter()
    fake_first = _FakeClient()
    lock = adapter._locks.setdefault("cx-dc", asyncio.Lock())
    await lock.acquire()
    task = asyncio.create_task(adapter.ensure_running("cx-dc"))
    await asyncio.sleep(0)  # deixa a task bloquear em `async with lock`
    adapter.attach("cx-dc", fake_first, "tid-dc")
    lock.release()
    with patch("app.adapters.codex.adapter.AppServerClient",
               lambda *a, **k: pytest.fail("nao deveria spawnar de novo")):
        client = await task
    assert client is fake_first


# --- Dead-detection: state_monitor emite "dead" quando o app-server morre --------------------

async def test_state_monitor_emits_dead_on_client_close():
    from app.state import StateEvent

    class _DyingClient:
        closed = False

        async def notifications(self):
            yield {"method": "turn/started", "params": {}}
            self.closed = True  # app-server morreu (EOF) apos essa notification
            return

    adapter = CodexAdapter()
    adapter.attach("sess", _DyingClient(), "t")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    assert events[-1] == StateEvent(session="sess", state="dead")
