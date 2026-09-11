"""Testes do CodexAdapter: map_state (notification -> estado/statusline/preview) + orquestracao
(state_monitor/send_prompt/deliverable/transcript_stream) contra um AppServerClient FAKE (sem
spawnar o codex real — o wire protocol ja e testado em test_codex_appserver.py)."""
import json

import pytest
from unittest.mock import patch

from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex.adapter import (
    CodexAdapter, ensure_tmux_tui, format_status_line, map_state,
)
from app.adapters.codex.preview import CodexPreviewSource
from app.state import StateEvent


@pytest.fixture(autouse=True)
def _isolate_sidecar(tmp_path):
    # Sidecars duraveis redirecionados pra tmp -- mesmo padrao de test_codex_registry.py (evita
    # os testes de model/effort tocarem ~/.hangar/codex-sessions de verdade).
    with patch.object(codex_sessions, "_dir", lambda: tmp_path / "codex-sessions"), \
         patch.object(codex_adapter, "ensure_tmux_tui"), \
         patch.object(codex_adapter.tmux, "has_session", return_value=True), \
         patch.object(codex_adapter.tmux, "sessao_existe", return_value=True), \
         patch.object(codex_adapter.tmux, "paste_text"), \
         patch.object(codex_adapter.tmux, "send_keys"):
        yield


class _FakeClient:
    """Duck-type minimo de AppServerClient: notifications() prefixadas + request() gravado."""

    def __init__(self, notifs: list[dict]):
        self._notifs = notifs
        self.requests: list[tuple[str, dict]] = []

    async def notifications(self):
        for n in self._notifs:
            yield n

    async def start(self):
        pass  # ensure_running (resume) chama start(); duck-type suficiente pros testes daqui

    async def start_shared(self):
        return "ws://127.0.0.1:45123"

    async def close(self):
        pass

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        self.requests.append((method, params))
        # turn/start devolve um Turn real -> expõe o turnId (necessario pro turn/interrupt).
        if method == "turn/start":
            return {"turn": {"id": "turn-fake"}}
        return {}


# --- map_state ---------------------------------------------------------------------------

def test_turn_started_working():
    assert map_state({"method": "turn/started", "params": {"threadId": "x"}}).state == "working"


def test_turn_completed_idle():
    assert map_state({"method": "turn/completed", "params": {"threadId": "x"}}).state == "idle"


def test_thread_status_changed_active_is_working():
    ev = {"method": "thread/status/changed", "params": {"status": {"type": "active"}}}
    assert map_state(ev).state == "working"


def test_thread_status_changed_idle():
    ev = {"method": "thread/status/changed", "params": {"status": {"type": "idle"}}}
    assert map_state(ev).state == "idle"


def test_token_usage_updated_captures_raw_snapshot():
    # Task D: map_state so captura o snapshot cru -- quem formata pro status_line completo e
    # format_status_line, chamado pelo _state_stream com o que estiver acumulado por sessao.
    usage = {"total": {"totalTokens": 1000}, "last": {"totalTokens": 1000},
             "modelContextWindow": 10000}
    ev = {"method": "thread/tokenUsage/updated", "params": {"threadId": "x", "turnId": "t",
          "tokenUsage": usage}}
    mapped = map_state(ev)
    assert mapped.token_usage == usage
    assert mapped.state is None and mapped.status_line is None


def test_agent_message_delta_is_preview():
    ev = {"method": "item/agentMessage/delta",
          "params": {"threadId": "x", "turnId": "t", "itemId": "i", "delta": "ok"}}
    r = map_state(ev)
    assert r.preview_delta == "ok"


def test_rate_limits_updated_captures_raw_snapshot():
    # Task D: antes era "unknown method = neutro" -- agora account/rateLimits/updated e
    # reconhecido e o snapshot cru (primary/secondary) e capturado pro _state_stream acumular.
    snapshot = {"limitId": "codex", "primary": {"usedPercent": 12, "windowDurationMins": 300,
                "resetsAt": 123}, "secondary": None}
    ev = {"method": "account/rateLimits/updated", "params": {"rateLimits": snapshot}}
    mapped = map_state(ev)
    assert mapped.rate_limits == snapshot
    assert mapped.state is None and mapped.status_line is None


def test_unknown_method_is_neutral():
    r = map_state({"method": "some/unmapped/method", "params": {}})
    assert (r.state is None and r.status_line is None and r.preview_delta is None
            and r.token_usage is None and r.rate_limits is None)


# --- format_status_line (Task D: formatador puro, casa os regexes do parseStatusLine do front) --

def test_format_status_line_full_example_matches_brief():
    # Reproduz o exemplo do brief: modelo+effort, 1 par turno zerado + 1 par de contexto, e so a
    # janela semanal (sem primary de 5h -- caso comum, so windowDurationMins=10080).
    now = 1_000_000.0
    # contexto usado = input do ULTIMO turno (14389), NAO o total acumulado.
    token_usage = {"total": {"totalTokens": 99999},
                    "last": {"inputTokens": 14389, "outputTokens": 0, "totalTokens": 14389},
                    "modelContextWindow": 258400}
    rate_limits = {"primary": {"usedPercent": 0, "windowDurationMins": 10080,
                    "resetsAt": now + 6 * 86400}}
    sl = format_status_line("GPT-5.5", "high", token_usage, rate_limits, now=now)
    assert sl == "🤖 GPT-5.5 (high) │ 💬 14k/0 14k/258k │ 📅7d:0% ↺6d"


def test_format_status_line_both_rate_windows():
    now = 1_000_000.0
    rate_limits = {
        "primary": {"usedPercent": 46, "windowDurationMins": 300, "resetsAt": now + 34 * 60},
        "secondary": {"usedPercent": 57, "windowDurationMins": 10080,
                      "resetsAt": now + 2 * 86400 + 3600},
    }
    sl = format_status_line(None, None, None, rate_limits, now=now)
    assert sl == "⚡5h:46% ↺34m │ 📅7d:57% ↺2d1h"


def test_format_status_line_omits_missing_sections():
    # so modelo, sem effort/token_usage/rate_limits -- as demais secoes somem, nao viram "│ │".
    assert format_status_line("gpt-5.5", None, None, None) == "🤖 gpt-5.5"


def test_format_status_line_all_missing_is_none():
    assert format_status_line(None, None, None, None) is None


def test_format_status_line_context_needs_total_and_window():
    # tokenUsage presente mas sem total/window utilizavel -> secao 💬 omitida (best-effort).
    sl = format_status_line("gpt-5.5", None, {"last": {"inputTokens": 1}}, None)
    assert sl == "🤖 gpt-5.5"


def test_format_status_line_matches_parse_status_line_regexes():
    # TDD: casa os MESMOS regexes que frontend/src/lib/statusline.ts::parseStatusLine usa, pra
    # garantir que o formato gerado e realmente entendido pelo front (sem rodar TS aqui).
    import re
    now = 1_000_000.0
    token_usage = {"total": {"totalTokens": 5000},
                   "last": {"inputTokens": 100, "outputTokens": 50, "totalTokens": 5000},
                   "modelContextWindow": 10000}
    rate_limits = {"primary": {"usedPercent": 46, "windowDurationMins": 300, "resetsAt": now + 34 * 60},
                   "secondary": {"usedPercent": 57, "windowDurationMins": 10080,
                                 "resetsAt": now + 2 * 86400 + 3600}}
    sl = format_status_line("GPT-5.5", "high", token_usage, rate_limits, now=now)

    model_re = re.compile(r"🤖\s*([^(│]+?)\s*(?:\(([^)]*)\))?\s*(?:👤|│|$)")
    m = model_re.search(sl)
    assert m and m.group(1).strip() == "GPT-5.5" and m.group(2) == "high"

    ctx_seg = re.search(r"💬([^│]*)", sl).group(1)
    pairs = re.findall(r"([\d.,]+)\s*([kKmM])?\s*/\s*([\d.,]+)\s*([kKmM])?", ctx_seg)
    assert len(pairs) >= 2  # o parser exige >=2 pares e usa o ultimo como contexto

    five_h = re.search(r"⚡[^│]*?(\d+)\s*%\s*(?:↺\s*([^│⚡📅🕐]+))?", sl)
    assert five_h and int(five_h.group(1)) == 46

    weekly = re.search(r"📅[^│]*?(\d+)\s*%\s*(?:↺\s*([^│🕐]+))?", sl)
    assert weekly and int(weekly.group(1)) == 57


# --- CodexAdapter.state_monitor ------------------------------------------------------------

async def test_state_monitor_dead_when_not_attached():
    adapter = CodexAdapter()
    events = [ev async for ev in adapter.state_monitor("ghost", lambda: "ghost")]
    assert events == [StateEvent(session="ghost", state="dead")]


async def test_state_monitor_emits_working_then_idle():
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {"threadId": "t"}},
        {"method": "turn/completed", "params": {"threadId": "t"}},
    ])
    adapter.attach("sess", client, "t")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    # events[0] e sempre o retrato do estado ao abrir (idle, sessao recem-ligada); daqui em diante
    # cada teste olha o que as notifications produziram.
    assert events[0].state == "idle"
    assert [e.state for e in events[1:]] == ["working", "idle"]


async def test_state_monitor_skips_preview_only_notifications():
    # item/agentMessage/delta nao vira StateEvent (StateEvent nao tem campo de preview) — a
    # notification e absorvida sem quebrar o stream nem emitir um evento vazio.
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "item/agentMessage/delta", "params": {"delta": "oi"}},
        {"method": "turn/completed", "params": {}},
    ])
    adapter.attach("sess", client, "t")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    assert [e.state for e in events[1:]] == ["idle"]


async def test_state_monitor_carries_status_line_without_changing_state():
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},
        {"method": "thread/tokenUsage/updated", "params": {
            "tokenUsage": {"total": {"totalTokens": 99999},
                           "last": {"inputTokens": 5000, "outputTokens": 0},
                           "modelContextWindow": 10000}}},
    ])
    adapter.attach("sess", client, "t")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    assert [e.state for e in events[1:]] == ["working", "working"]  # segue o ultimo estado conhecido
    # sem model/effort escolhidos (attach sem eles) -> so a secao de contexto aparece.
    # contexto = input do ultimo turno (5000), nao o total acumulado.
    assert events[2].status_line == "💬 5k/0 5k/10k"


async def test_state_monitor_accumulates_token_usage_and_rate_limits_across_events():
    # Task D: tokenUsage/rateLimits sao notifications esparsas -- uma vez recebidas, TODO
    # StateEvent seguinte (mesmo working/idle puro, sem token/limite novo) carrega o snapshot
    # mais recente acumulado, junto com model/effort ja anexados via attach().
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},
        {"method": "thread/tokenUsage/updated", "params": {
            "tokenUsage": {"total": {"totalTokens": 1000},
                           "last": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 1000},
                           "modelContextWindow": 10000}}},
        {"method": "account/rateLimits/updated", "params": {
            "rateLimits": {"primary": {"usedPercent": 20, "windowDurationMins": 10080,
                           "resetsAt": 0}}}},
        {"method": "turn/completed", "params": {}},
    ])
    adapter.attach("sess", client, "t", model="gpt-5.5", effort="high")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    last = events[-1]  # turn/completed -> idle, SEM token/limite novo neste notif
    assert last.state == "idle"
    assert last.status_line.startswith("🤖 gpt-5.5 (high) │ 💬 10/5 10/10k │ 📅7d:20%")


async def test_state_monitor_accumulates_deltas_into_preview_source():
    # item/agentMessage/delta e INCREMENTAL (docs/codex-app-server-contract.md: "o","k" -> "ok").
    # state_monitor acumula no buffer do turno e empurra pro CodexPreviewSource -- efeito colateral
    # ADICIONAL aos StateEvent (working/idle), que continuam saindo como antes (Task 4).
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {"threadId": "t"}},
        {"method": "item/agentMessage/delta", "params": {"delta": "o"}},
        {"method": "item/agentMessage/delta", "params": {"delta": "k"}},
        {"method": "turn/completed", "params": {"threadId": "t"}},
    ])
    adapter.attach("sess-preview", client, "t")
    events = [ev async for ev in adapter.state_monitor("sess-preview", lambda: "sess-preview")]
    assert [e.state for e in events[1:]] == ["working", "idle"]  # StateEvents intactos (nao regrediu)
    # o preview foi empurrado a cada delta (visivel via subscribe: "o" depois "ok") e limpo no fim.
    assert CodexPreviewSource.get("sess-preview").text == ""  # turn/completed -> push("") limpa


async def test_state_monitor_pushes_incremental_deltas_before_clearing():
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {"threadId": "t"}},
        {"method": "item/agentMessage/delta", "params": {"delta": "o"}},
    ])
    adapter.attach("sess-preview2", client, "t")
    events = [ev async for ev in adapter.state_monitor("sess-preview2", lambda: "sess-preview2")]
    assert [e.state for e in events[1:]] == ["working"]
    assert CodexPreviewSource.get("sess-preview2").text == "o"  # sem turn/completed, nao limpou


async def test_state_monitor_resets_buffer_on_new_turn_started():
    # 1o turno acumula "ok" e completa (limpa); 2o turno comeca do zero -- sem isto, um delta "!"
    # sozinho no 2o turno viraria "ok!" (vazamento do buffer do turno anterior).
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},
        {"method": "item/agentMessage/delta", "params": {"delta": "o"}},
        {"method": "item/agentMessage/delta", "params": {"delta": "k"}},
        {"method": "turn/completed", "params": {}},
        {"method": "turn/started", "params": {}},
        {"method": "item/agentMessage/delta", "params": {"delta": "!"}},
    ])
    adapter.attach("sess-preview3", client, "t")
    async for _ in adapter.state_monitor("sess-preview3", lambda: "sess-preview3"):
        pass
    assert CodexPreviewSource.get("sess-preview3").text == "!"


# --- CodexAdapter.send_prompt / deliverable -------------------------------------------------

async def test_send_prompt_deferred_when_not_attached():
    adapter = CodexAdapter()
    assert await adapter.send_prompt("ghost", "oi") == "deferred"


async def test_warm_sessions_reconnects_all_sidecars_without_stopping_on_error(monkeypatch):
    adapter = CodexAdapter()
    monkeypatch.setattr(codex_sessions, "list_all", lambda: [
        {"name": "um"}, {"name": "dois"}, {"sem_nome": True},
    ])
    seen = []

    async def ensure(name):
        seen.append(name)
        if name == "um":
            raise RuntimeError("fora")

    monkeypatch.setattr(adapter, "ensure_running", ensure)
    await adapter.warm_sessions()
    assert seen == ["um", "dois"]


async def test_warm_descobre_nova_sessao_sem_reconectar_as_existentes(monkeypatch):
    from types import SimpleNamespace
    adapter = CodexAdapter()
    metas = [{"name": "um", "thread_id": "thread-um"}]
    monkeypatch.setattr(codex_sessions, "list_all", lambda: list(metas))
    seen = []
    async def ensure(name):
        seen.append(name)
        adapter._sessions[name] = {"thread_id": "thread-" + name, "client": SimpleNamespace(closed=False)}
    monkeypatch.setattr(adapter, "ensure_running", ensure)
    await adapter.warm_sessions()
    metas.append({"name": "dois", "thread_id": "thread-dois"})
    await adapter.warm_sessions()
    assert seen == ["um", "dois"]


async def test_send_prompt_uses_turn_start_not_tmux(monkeypatch):
    # O prompt vai por turn/start no app-server, NAO digitado no pane. Medido (probe contra
    # codex-cli 0.144.6): a TUI `codex --remote` renderiza turno iniciado por outro cliente, entao
    # o terminal continua vendo a msg do celular sem o backend encostar no terminal_input (que e
    # do caminho Claude).
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    typed = []
    monkeypatch.setattr(codex_adapter.tmux, "send_keys",
                        lambda name, key: typed.append((name, key)))
    assert await adapter.send_prompt("sess", "oi") == "sent"
    assert client.requests == [
        ("turn/start", {"threadId": "thread-1", "input": [{"type": "text", "text": "oi"}]}),
    ]
    assert typed == []   # nada digitado no tmux


async def test_send_prompt_marks_in_progress_on_sent():
    # send_prompt tem que setar in_progress=True ao entregar -- senao deliverable() continua
    # True logo em seguida e um drain com varias entradas pendentes as manda todas como turn/start
    # concorrentes (o turn/started do 1o envio so seria processado depois, no loop de notifications).
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    assert await adapter.send_prompt("sess", "oi") == "sent"
    assert await adapter.deliverable("sess") is False


async def test_deliverable_true_when_untracked():
    adapter = CodexAdapter()
    assert await adapter.deliverable("ghost") is True


async def test_deliverable_false_during_turn():
    adapter = CodexAdapter()
    client = _FakeClient([{"method": "turn/started", "params": {}}])
    adapter.attach("sess", client, "t")
    async for _ in adapter.state_monitor("sess", lambda: "sess"):
        pass
    assert await adapter.deliverable("sess") is False


# --- CodexAdapter.interrupt (turn/interrupt) ------------------------------------------------

async def test_interrupt_calls_turn_interrupt():
    # Interrupt vai pelo app-server (threadId+turnId), nao por Escape no pane: vale igual venha
    # do celular ou do terminal, e nao depende de heuristica de TUI.
    adapter = CodexAdapter()
    client = _FakeClient([{"method": "turn/started",
                           "params": {"turn": {"id": "turn-9"}}}])
    adapter.attach("sess", client, "thread-1")
    async for _ in adapter.state_monitor("sess", lambda: "sess"):
        pass   # consome o turn/started -> guarda o turn_id em voo
    assert await adapter.interrupt("sess") is True
    assert ("turn/interrupt", {"threadId": "thread-1", "turnId": "turn-9"}) in client.requests


async def test_interrupt_noop_when_not_attached():
    adapter = CodexAdapter()
    assert await adapter.interrupt("ghost") is False


async def test_interrupt_noop_when_no_turn_in_flight():
    # Sem turno em voo nao ha o que interromper: no-op seguro, sem mandar RPC de turno morto.
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    assert await adapter.interrupt("sess") is False
    assert client.requests == []


async def test_interrupt_reconnects_cold_session_before_reading_turn(monkeypatch):
    adapter = CodexAdapter()
    client = _FakeClient([])
    calls = []

    async def ensure(name):
        calls.append(name)
        adapter._sessions[name] = {
            "client": client, "thread_id": "thread-1", "in_progress": True,
            "turn_id": "turn-1", "state": "working",
        }
        return client

    monkeypatch.setattr(adapter, "ensure_running", ensure)
    assert await adapter.interrupt("fria") is True
    assert calls == ["fria"]
    assert ("turn/interrupt", {"threadId": "thread-1", "turnId": "turn-1"}) in client.requests


# --- CodexAdapter drain-on-complete (P2) ----------------------------------------------------

async def test_state_monitor_drains_queue_on_turn_completed(monkeypatch):
    # turn/completed dispara adapter.drain -> a fila pendente (msgs enviadas durante o working) e
    # entregue quando o turno termina. Aqui so verifica o WIRING (drain mockado).
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},
        {"method": "turn/completed", "params": {}},
    ])
    adapter.attach("sess-drain", client, "t")
    calls = []

    async def fake_drain(name, path):
        calls.append((name, path))
        return 0

    monkeypatch.setattr(adapter, "drain", fake_drain)
    async for _ in adapter.state_monitor("sess-drain", lambda: "sess-drain"):
        pass
    assert calls == [("sess-drain", "")]


async def test_turn_completed_delivers_queue_without_idle_status(monkeypatch, tmp_path):
    # Regressao (ordenacao de notification): turn/completed deve marcar idle ANTES de drenar. Aqui a
    # sequencia e turn/started -> turn/completed SEM um thread/status/changed idle no meio. Sem o fix,
    # in_progress fica True na hora da drain -> send_prompt="deferred" -> a entrada enfileirada durante
    # o working nunca e entregue (perda silenciosa). Com o fix, ela sai via turn/start. Drain REAL
    # (nao mockado) pra provar a ENTREGA, nao so o wiring.
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", tmp_path / "projects")
    from app.pqueue import PromptQueue
    q = PromptQueue("sess-realdrain")
    q.append("msg pendente", delivered=False)

    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},    # in_progress -> True
        {"method": "turn/completed", "params": {}},   # SEM idle-status antes: tem que drenar assim mesmo
    ])
    adapter.attach("sess-realdrain", client, "thread-x")
    async for _ in adapter.state_monitor("sess-realdrain", lambda: "sess-realdrain"):
        pass

    starts = [r for r in client.requests if r[0] == "turn/start"]
    assert len(starts) == 1  # a entrada pendente foi entregue via turn/start
    assert starts[0][1]["input"][0]["text"] == "msg pendente"
    assert all(e["delivered"] for e in q.load())  # nao ficou presa na fila


async def test_drain_stops_after_first_delivery_with_two_pending(monkeypatch, tmp_path):
    # Fix 2 (via drain): com 2 entradas pendentes e deliverable inicial True, o drain so pode
    # entregar 1 (turn/start) -- a 2a fica presa ate o proximo turn/completed. Sem o fix, in_progress
    # nunca vira True apos o 1o envio e as 2 saem back-to-back como turn/start concorrentes.
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", tmp_path / "projects")
    from app.pqueue import PromptQueue
    q = PromptQueue("sess-drain2")
    q.append("msg 1", delivered=False)
    q.append("msg 2", delivered=False)

    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess-drain2", client, "thread-y")
    sent = await adapter.drain("sess-drain2", "")

    starts = [r for r in client.requests if r[0] == "turn/start"]
    assert len(starts) == 1
    assert sent == 1
    assert sum(1 for e in q.load() if e["delivered"] is False) == 1


# --- CodexAdapter.transcript_stream (reaproveita TranscriptTailer + parse_rollout_line) ------

async def test_drain_reverts_claim_when_send_prompt_raises(monkeypatch, tmp_path):
    # CRITICAL: claim_undelivered ja marcou delivered=True (otimista) antes do send_prompt. Se
    # send_prompt LEVANTA (app-server morto/timeout/RuntimeError do JSON-RPC), o except tinha que
    # reverter igual ao branch "deferred" -- sem isto a entrada ficava delivered=True pra sempre
    # (bolha "queued-" eterna, nunca reenviada = perda silenciosa).
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", tmp_path / "projects")
    from app.pqueue import PromptQueue
    q = PromptQueue("sess-drainexc")
    q.append("msg pendente", delivered=False)

    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess-drainexc", client, "thread-z")

    async def boom(name, text):
        raise RuntimeError("app-server morreu")

    monkeypatch.setattr(adapter, "send_prompt", boom)
    sent = await adapter.drain("sess-drainexc", "")  # nao pode crashar

    assert sent == 0
    assert all(e["delivered"] is False for e in q.load())  # reivindicavel de novo


async def test_transcript_stream_parses_rollout_lines(tmp_path):
    f = tmp_path / "rollout.jsonl"
    f.write_text(json.dumps({
        "type": "response_item",
        "payload": {"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": "oi"}]},
    }) + "\n")
    adapter = CodexAdapter()
    got = []
    async for ev in adapter.transcript_stream(str(f)):
        got.append(ev)
        break
    assert got[0].kind == "user_msg"


# --- CodexAdapter.read_rate_limits (Task B) -------------------------------------------------

async def test_read_rate_limits_returns_snapshot():
    adapter = CodexAdapter()
    snapshot = {
        "limitId": "codex", "limitName": None,
        "primary": {"usedPercent": 42, "windowDurationMins": 10080, "resetsAt": 1784494806},
        "secondary": None, "credits": None, "individualLimit": None,
        "planType": "plus", "rateLimitReachedType": None,
    }

    class _RateClient(_FakeClient):
        async def request(self, method, params, timeout=30.0):
            self.requests.append((method, params))
            if method == "account/rateLimits/read":
                return {"rateLimits": snapshot}
            return {}

    client = _RateClient([])
    adapter.attach("sess", client, "thread-1")
    got = await adapter.read_rate_limits("sess")
    assert got == snapshot
    assert "📅7d:42%" in adapter._status_line(adapter._sessions["sess"])
    assert ("account/rateLimits/read", {}) in client.requests


def test_spark_quota_does_not_replace_account_quota():
    mapped = map_state({"method": "account/rateLimits/updated", "params": {
        "rateLimits": {"limitId": "codex_bengalfox", "primary": {
            "usedPercent": 0, "windowDurationMins": 10080}}}})
    assert mapped.rate_limits is None


def test_rollout_uses_account_quota_before_latest_spark_snapshot(tmp_path):
    path = tmp_path / "rollout.jsonl"
    rows = [
        {"type": "turn_context", "payload": {"model": "gpt-6-astra"}},
        {"type": "event_msg", "payload": {"type": "token_count", "rate_limits": {
            "limit_id": "codex", "primary": {"used_percent": 25, "window_minutes": 10080}}}},
        {"type": "event_msg", "payload": {"type": "token_count", "rate_limits": {
            "limit_id": "codex_bengalfox", "primary": {"used_percent": 0, "window_minutes": 300},
            "secondary": {"used_percent": 0, "window_minutes": 10080}},
            "info": {"last_token_usage": {"input_tokens": 42000, "output_tokens": 100},
                     "model_context_window": 872000}}},
    ]
    path.write_text("\n".join(json.dumps(row) for row in rows))
    line = codex_adapter.status_line_do_rollout(str(path))
    assert "📅7d:25%" in line and "⚡5h" not in line
    assert "42k" in line


async def test_read_rate_limits_none_when_not_attached():
    # sem client vivo e sem sidecar (nome desconhecido) -> None, nunca levanta.
    adapter = CodexAdapter()
    assert await adapter.read_rate_limits("ghost") is None


async def test_read_rate_limits_none_when_request_raises():
    adapter = CodexAdapter()

    class _BoomClient(_FakeClient):
        async def request(self, method, params, timeout=30.0):
            raise RuntimeError("app-server recusou")

    adapter.attach("sess", _BoomClient([]), "thread-1")
    assert await adapter.read_rate_limits("sess") is None


# --- registro em PROVIDERS -------------------------------------------------------------------

def test_codex_registered_in_providers():
    from app.adapters import PROVIDERS
    assert isinstance(PROVIDERS["codex"], CodexAdapter)


# --- CodexAdapter.list_models / set_model / current_model (Task C) -------------------------

_MODEL_LIST_RESULT = {
    "data": [
        {
            "id": "gpt-5-codex", "model": "gpt-5-codex", "displayName": "GPT-5 Codex",
            "description": "modelo padrao", "hidden": False,
            "supportedReasoningEfforts": [
                {"reasoningEffort": "low", "description": "mais rapido"},
                {"reasoningEffort": "high", "description": "mais capaz"},
            ],
            "defaultReasoningEffort": "medium",
        },
        {
            "id": "gpt-5-legacy", "model": "gpt-5-legacy", "displayName": "GPT-5 (legacy)",
            "description": "descontinuado", "hidden": True,
            "supportedReasoningEfforts": [], "defaultReasoningEffort": None,
        },
    ],
}


class _ModelListClient(_FakeClient):
    async def request(self, method, params, timeout=30.0):
        self.requests.append((method, params))
        if method == "model/list":
            return _MODEL_LIST_RESULT
        return {}


async def test_list_models_filters_hidden_and_normalizes():
    adapter = CodexAdapter()
    client = _ModelListClient([])
    adapter.attach("sess", client, "thread-1")
    models = await adapter.list_models("sess")
    assert models == [{
        "model": "gpt-5-codex", "displayName": "GPT-5 Codex", "description": "modelo padrao",
        "efforts": [
            {"value": "low", "description": "mais rapido"},
            {"value": "high", "description": "mais capaz"},
        ],
        "defaultEffort": "medium",
    }]  # o hidden=True foi filtrado
    assert ("model/list", {}) in client.requests


async def test_list_models_empty_when_not_attached():
    adapter = CodexAdapter()
    assert await adapter.list_models("ghost") == []


async def test_list_models_empty_when_request_raises():
    adapter = CodexAdapter()

    class _BoomClient(_FakeClient):
        async def request(self, method, params, timeout=30.0):
            raise RuntimeError("app-server recusou")

    adapter.attach("sess", _BoomClient([]), "thread-1")
    assert await adapter.list_models("sess") == []


async def test_set_model_updates_dict_and_sidecar():
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    assert adapter._sessions["sess"]["model"] == "gpt-5-codex"
    assert adapter._sessions["sess"]["effort"] == "high"
    saved = codex_sessions.load("sess")
    assert saved["model"] == "gpt-5-codex"
    assert saved["effort"] == "high"
    # thread_id/rollout_path/cwd preservados -- set_model nao pode corromper a identidade da sessao
    assert saved["thread_id"] == "thread-1"
    assert saved["rollout_path"] == "/rollout.jsonl"


async def test_set_model_noop_on_sidecar_when_never_saved():
    # sessao anexada so em memoria (sem sidecar ainda) -- update_model nao deve levantar.
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1")
    await adapter.set_model("sess", "gpt-5-codex", None)
    assert adapter._sessions["sess"]["model"] == "gpt-5-codex"
    assert codex_sessions.load("sess") is None


async def test_current_model_from_dict_when_set():
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high"}


async def test_current_model_falls_back_to_sidecar_when_not_attached():
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj",
                         model="gpt-5-codex", effort="low")
    adapter = CodexAdapter()
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "low"}


async def test_current_model_null_when_never_chosen():
    adapter = CodexAdapter()
    assert adapter.current_model("ghost") == {"model": None, "effort": None}


# --- modelo/effort viajam no proprio turn/start ---------------------------------------------

async def test_send_prompt_herda_configuracao_viva_sem_sobrescrever_terminal():
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    await adapter.send_prompt("sess", "oi")
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high"}
    assert client.requests == [("thread/settings/update", {
        "threadId": "thread-1", "model": "gpt-5-codex", "effort": "high",
    }), ("turn/start", {
        "threadId": "thread-1", "input": [{"type": "text", "text": "oi"}],
    })]


async def test_set_model_does_not_restart_tui(monkeypatch):
    # Guard da corrida: set_model NAO pode chamar ensure_tmux_tui (kill_session + new_session).
    # Entre o kill e o new, o watcher de 1s via a sessao sumida e apagava sidecar + fila + app-server.
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1")
    called = []
    monkeypatch.setattr(codex_adapter, "ensure_tmux_tui",
                        lambda *a, **k: called.append(a))
    await adapter.set_model("sess", "gpt-5-codex", "high")
    assert called == []


async def test_send_prompt_omits_model_effort_when_unset():
    # Sem escolha explicita, nao manda os campos -> a thread usa o proprio default.
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.send_prompt("sess", "oi")
    assert client.requests == [
        ("turn/start", {"threadId": "thread-1", "input": [{"type": "text", "text": "oi"}]}),
    ]


async def test_ensure_running_resume_restores_model_effort_from_sidecar():
    # Pos-restart: dict vazio, sidecar tem a escolha gravada antes do restart. ensure_running
    # (resume) tem que repovoar model/effort no dict quente -- senao o proximo turn/start
    # "esqueceria" a escolha ate o usuario reabrir o picker.
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj",
                         model="gpt-5-codex", effort="high")
    adapter = CodexAdapter()
    client = _FakeClient([])
    # has_session False: este resume RECRIA a TUI, e recriar so e permitido quando nao ha pane —
    # com pane vivo, a resposta certa e nao mexer (ver test_pane_vivo_sem_controle_nunca_e_substituido).
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: client), \
         patch.object(codex_adapter.tmux, "has_session", return_value=False):
        await adapter.ensure_running("sess")
    assert adapter._sessions["sess"]["model"] == "gpt-5-codex"
    assert adapter._sessions["sess"]["effort"] == "high"
    await adapter.send_prompt("sess", "oi")
    assert client.requests[-1] == ("turn/start", {
        "threadId": "thread-1", "input": [{"type": "text", "text": "oi"}],
    })


# --- default_model (fix-model-display): display cai pro default da thread sem escolha ------

async def test_current_model_falls_back_to_default_when_no_explicit_choice():
    # attach() so com default_model (thread/start sem escolha do usuario) -> current_model
    # (usado pelo GET /models e pelo pill do front) mostra o default, nao None.
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1", default_model="gpt-5.6-sol")
    assert adapter.current_model("sess") == {"model": "gpt-5.6-sol", "effort": None}


async def test_current_model_explicit_choice_wins_over_default():
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1", default_model="gpt-5.6-sol")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high"}


async def test_send_prompt_omits_default_model_even_when_present():
    # default_model e so DISPLAY (o default que a propria thread ja usa). Mandar de volta no
    # turn/start seria transformar exibicao em escolha explicita -> so a escolha do usuario viaja.
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1", default_model="gpt-5.6-sol", default_effort="high")
    await adapter.send_prompt("sess", "oi")
    assert client.requests == [
        ("turn/start", {"threadId": "thread-1", "input": [{"type": "text", "text": "oi"}]}),
    ]


async def test_status_line_shows_default_model_when_no_explicit_choice():
    # 🤖 no statusline usa model-or-default (Task fix-model-display) -- sem isso o pill/status
    # ficavam genericos ate o usuario escolher, mesmo com um modelo default rodando de verdade.
    adapter = CodexAdapter()
    client = _FakeClient([{"method": "turn/completed", "params": {}}])
    adapter.attach("sess", client, "t", default_model="gpt-5.6-sol")
    events = [e async for e in adapter.state_monitor("sess", lambda: "sess")]
    assert any(e.status_line and "🤖 gpt-5.6-sol" in e.status_line for e in events)


async def test_ensure_running_resume_captures_default_without_overwriting_choice():
    # Pos-restart: sidecar tem a escolha explicita gravada; thread/resume devolve o `model` da
    # thread (default) -- ensure_running tem que popular default_model SEM pisar na escolha.
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj",
                         model="gpt-5-codex", effort="high")
    adapter = CodexAdapter()

    class _ResumeClient(_FakeClient):
        async def request(self, method, params, timeout=30.0):
            self.requests.append((method, params))
            if method == "thread/resume":
                return {"thread": {"id": "thread-1"}, "model": "gpt-5.6-sol"}
            return {}

    client = _ResumeClient([])
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: client), \
         patch.object(codex_adapter.tmux, "has_session", return_value=False):
        await adapter.ensure_running("sess")
    sess = adapter._sessions["sess"]
    assert sess["model"] == "gpt-5-codex"       # escolha preservada
    assert sess["default_model"] == "gpt-5.6-sol"  # default capturado, so pra display
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high"}


# O campo do esforco na resposta de thread/start|resume chama `reasoningEffort`, e nao `effort`
# como o parametro do turn/start (medido no codex-cli 0.153.4). Lendo `effort` a pilula do app
# nascia VAZIA com o terminal mostrando `max`, e o modelo aparecia -- ele vem da mesma resposta.
async def test_resume_le_o_esforco_da_thread_em_reasoning_effort():
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj")
    adapter = CodexAdapter()

    class _ResumeClient(_FakeClient):
        async def request(self, method, params, timeout=30.0):
            self.requests.append((method, params))
            if method == "thread/resume":
                return {"thread": {"id": "thread-1"}, "model": "gpt-5.6-luna",
                        "reasoningEffort": "max"}
            return {}

    client = _ResumeClient([])
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: client), \
         patch.object(codex_adapter.tmux, "has_session", return_value=False):
        await adapter.ensure_running("sess")
    assert adapter.current_model("sess") == {"model": "gpt-5.6-luna", "effort": "max"}


def test_transcript_stream_creates_rollout_dir(tmp_path):
    # 1a sessao Codex do dia: o dir do rollout (~/.codex/sessions/YYYY/MM/DD) ainda nao existe quando o
    # SSE abre o tail -> sem o mkdir, awatch(parent) em follow() derruba o SSE com FileNotFoundError.
    from app.adapters.codex.adapter import CodexAdapter
    rollout = tmp_path / "2026" / "07" / "14" / "rollout-x.jsonl"
    assert not rollout.parent.exists()
    CodexAdapter().transcript_stream(str(rollout))   # a chamada sync ja roda o mkdir (antes do return)
    assert rollout.parent.exists()


def test_ensure_tmux_tui_starts_remote_codex_for_thread():
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter.tmux, "new_session", return_value=True) as new_session:
        ensure_tmux_tui("cx", "/tmp/proj", "thread-42", "ws://127.0.0.1:45123")
    name, cwd, command = new_session.call_args.args
    assert (name, cwd) == ("cx", "/tmp/proj")
    assert command == (
        "codex resume --remote ws://127.0.0.1:45123 --no-alt-screen thread-42"
    )


def test_ensure_tmux_tui_resumes_with_model_and_effort():
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter.tmux, "new_session", return_value=True) as new_session:
        ensure_tmux_tui(
            "cx", "/tmp/proj", "thread-42", "ws://127.0.0.1:45123",
            model="gpt-5-codex", effort="high",
        )
    assert new_session.call_args.args[2] == (
        "codex resume --remote ws://127.0.0.1:45123 --no-alt-screen "
        "--model gpt-5-codex --config 'model_reasoning_effort=\"high\"' thread-42"
    )


def test_ensure_tmux_tui_creates_thread_with_matching_permissions():
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter.tmux, "new_session", return_value=True) as new_session:
        ensure_tmux_tui("cx", "/tmp/proj", None, "ws://127.0.0.1:45123")
    command = new_session.call_args.args[2]
    assert command == (
        "codex --remote ws://127.0.0.1:45123 --no-alt-screen -C /tmp/proj "
        "--sandbox danger-full-access --ask-for-approval never"
    )


def test_ensure_tmux_tui_forwards_initial_prompt_as_single_argument():
    with patch.object(codex_adapter.tmux, "has_session", return_value=False), \
         patch.object(codex_adapter.tmux, "new_session", return_value=True) as new_session:
        ensure_tmux_tui("cx", "/tmp/proj", None, "ws://127.0.0.1:45123",
                        initial_prompt="revise docs; sem executar shell")
    assert new_session.call_args.args[2].endswith(
        "'revise docs; sem executar shell'"
    )


def test_ensure_tmux_tui_replaces_stale_remote_after_backend_restart():
    with patch.object(codex_adapter.tmux, "has_session", return_value=True), \
         patch.object(codex_adapter.tmux, "kill_session") as kill, \
         patch.object(codex_adapter.tmux, "new_session", return_value=True):
        ensure_tmux_tui("cx", "/tmp/proj", "thread-42", "ws://127.0.0.1:45123",
                        replace=True)
    kill.assert_called_once_with("cx")


# --- assinatura da thread (thread/resume com retry) -----------------------------------------
# As notifications do app-server sao POR ASSINATURA, nao broadcast (MEDIDO contra codex-cli
# 0.144.6): sem thread/resume o backend so recebe eventos globais -- nada de turn/*, item/* ou
# tokenUsage. Era essa surdez que congelava o estado, matava o preview e prendia a fila do
# celular (o drain-on-complete mora no turn/completed). E ela nao pode ser feita na criacao:
# enquanto a thread nao tem turno o rollout nao existe e o app-server responde "no rollout found
# for thread id" -- daí o retry.
import asyncio


class _ResumeFailsThenWorks(_FakeClient):
    def __init__(self, failures: int):
        super().__init__([])
        self.left = failures

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        if method == "thread/resume" and self.left > 0:
            self.left -= 1
            raise RuntimeError("no rollout found for thread id")
        return await super().request(method, params, timeout)


def _fast_subscribe(adapter):
    adapter.SUBSCRIBE_RETRY_BASE = 0.01
    adapter.SUBSCRIBE_RETRY_MAX = 0.01
    return adapter


async def test_subscription_retries_until_rollout_exists():
    adapter = _fast_subscribe(CodexAdapter())
    client = _ResumeFailsThenWorks(failures=3)
    adapter.attach("sess", client, "thread-1")
    assert adapter._sessions["sess"]["subscribed"] is False
    adapter.start_subscription("sess", "/tmp/proj")
    await asyncio.wait_for(adapter._subscribers["sess"], timeout=5)
    assert adapter._sessions["sess"]["subscribed"] is True
    assert client.left == 0                      # retentou ate o rollout existir
    assert ("thread/resume", {
        "threadId": "thread-1", "cwd": "/tmp/proj",
        "sandbox": codex_adapter.SANDBOX, "approvalPolicy": codex_adapter.APPROVAL,
    }) in client.requests


async def test_subscription_stops_when_session_dies():
    # Sessao morre no meio do retry -> a task TERMINA, nao martela o app-server pra sempre.
    adapter = _fast_subscribe(CodexAdapter())
    adapter.attach("sess", _ResumeFailsThenWorks(failures=10**6), "thread-1")
    adapter.start_subscription("sess", "/tmp/proj")
    await asyncio.sleep(0.05)
    adapter._sessions.pop("sess")
    await asyncio.wait_for(adapter._subscribers["sess"], timeout=5)


async def test_close_sync_cancels_subscription():
    adapter = _fast_subscribe(CodexAdapter())
    adapter.attach("sess", _ResumeFailsThenWorks(failures=10**6), "thread-1")
    adapter.start_subscription("sess", "/tmp/proj")
    task = adapter._subscribers["sess"]
    adapter.close_sync("sess")
    assert "sess" not in adapter._subscribers
    with pytest.raises(asyncio.CancelledError):
        await task


async def test_ensure_running_marks_subscribed_without_retry_task():
    # Pos-restart o rollout ja existe -> o thread/resume do ensure_running cola de primeira e a
    # sessao ja nasce assinada; nao faz sentido armar a task de retry.
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj")
    adapter = CodexAdapter()
    client = _FakeClient([])
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: client), \
         patch.object(codex_adapter.tmux, "has_session", return_value=False):
        await adapter.ensure_running("sess")
    assert adapter._sessions["sess"]["subscribed"] is True
    assert "sess" not in adapter._subscribers


async def test_subscription_populates_default_model_for_display():
    # O thread/resume devolve o default da THREAD; ele alimenta o 🤖 do pill/statusline. Com
    # setdefault isto NAO funcionava (attach ja cria a chave com None) e o modelo sumia da
    # statusline -- pego na verificacao ao vivo, nao no teste.
    class _ResumeWithModel(_FakeClient):
        async def request(self, method, params, timeout=30.0):
            await super().request(method, params, timeout)
            if method == "thread/resume":
                return {"model": "gpt-5.6-sol", "effort": "medium"}
            return {}

    adapter = _fast_subscribe(CodexAdapter())
    adapter.attach("sess", _ResumeWithModel([]), "thread-1")
    adapter.start_subscription("sess", "/tmp/proj")
    await asyncio.wait_for(adapter._subscribers["sess"], timeout=5)
    assert adapter.current_model("sess") == {"model": "gpt-5.6-sol", "effort": "medium"}


async def test_rename_rearma_assinatura_pendente():
    # rename() cancelava a task de assinatura e NAO re-armava: renomear antes do 1o turno deixava
    # a sessao surda pra sempre (sem turn/*, sem preview, fila do celular presa) — nenhum outro
    # caminho reassina, porque ensure_running so atua quando NAO ha client vivo.
    codex_sessions.save("novo", "thread-1", "/rollout.jsonl", "/tmp/proj")
    adapter = _fast_subscribe(CodexAdapter())
    client = _ResumeFailsThenWorks(failures=1)
    adapter.attach("velho", client, "thread-1")
    adapter.start_subscription("velho", "/tmp/proj")
    adapter.rename("velho", "novo")
    assert "novo" in adapter._subscribers
    await asyncio.wait_for(adapter._subscribers["novo"], timeout=5)
    assert adapter._sessions["novo"]["subscribed"] is True


async def test_rename_nao_reassina_sessao_ja_assinada():
    adapter = _fast_subscribe(CodexAdapter())
    adapter.attach("velho", _FakeClient([]), "thread-1", subscribed=True)
    adapter.rename("velho", "novo")
    assert "novo" not in adapter._subscribers


async def test_rename_sync_rearma_bomba_no_loop_do_backend():
    codex_sessions.save("novo", "thread-1", "/rollout.jsonl", "/tmp/proj")
    adapter = _fast_subscribe(CodexAdapter())
    client = _LiveQueueClient()
    adapter.attach("velho", client, "thread-1", subscribed=True)
    antiga = adapter._sessions["velho"]["bomba"]

    await asyncio.to_thread(adapter.rename, "velho", "novo")
    async with asyncio.timeout(1):
        while adapter._sessions["novo"].get("bomba") is antiga:
            await asyncio.sleep(0)
        while not antiga.done():
            await asyncio.sleep(0)

    assert antiga.cancelled()
    assert client.aberturas == 2
    await client._q.put(None)
    await adapter._sessions["novo"]["bomba"]


@pytest.mark.parametrize("blocked_method,fail", [("skills/list", False), ("turn/start", False), ("skills/list", True)])
async def test_rename_espera_a_entrega_reivindicada(tmp_path, monkeypatch, blocked_method, fail):
    from app import api, pqueue
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    entered, release = asyncio.Event(), asyncio.Event()

    class Client(_LiveQueueClient):
        async def request(self, method, params, timeout=30.0):
            result = await super().request(method, params, timeout)
            if method == blocked_method:
                entered.set()
                await release.wait()
                if fail:
                    raise RuntimeError("falha antes do envio")
            return result

    client = Client()
    adapter = CodexAdapter()
    adapter.attach("velho", client, "thread-1", subscribed=True)
    monkeypatch.setattr(api, "get_adapter", lambda _provider: adapter)
    renamed = []

    def rename(name, body):
        renamed.append(name)
        adapter.rename(name, body.new)
        pqueue.PromptQueue(name).rename(body.new)
        return {"ok": True}

    monkeypatch.setattr(api, "_rename_session", rename)
    pqueue.PromptQueue("velho").append("/review investigar bug")
    await client._q.put({"method": "turn/completed", "params": {"threadId": "thread-1"}})
    async with asyncio.timeout(3):
        await entered.wait()
        task = asyncio.create_task(api.rename_session("velho", api.RenameBody(new="novo")))
        await asyncio.sleep(0)
        assert renamed == []
        assert pqueue.PromptQueue("velho").load()[0]["delivered"] is True
        release.set()
        await task
    assert len([method for method, _ in client.requests if method == "turn/start"]) == (0 if fail else 1)
    assert pqueue.PromptQueue("novo").load()[0]["delivered"] is (not fail)
    assert pqueue.PromptQueue("velho").load() == []
    await client._q.put(None)
    await adapter._sessions["novo"]["bomba"]


async def test_rename_protege_fila_do_destino_e_permite_voltar(tmp_path, monkeypatch):
    import threading
    from app import api, pqueue
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    adapter = CodexAdapter()
    monkeypatch.setattr(api, "get_adapter", lambda _provider: adapter)
    monkeypatch.setattr(api, "_session_exists", lambda _name: True)
    monkeypatch.setattr(adapter, "deliverable", lambda _name: asyncio.sleep(0, result=False))
    entered, release = threading.Event(), threading.Event()

    def rename(name, body):
        entered.set()
        assert release.wait(3)
        pqueue.PromptQueue(name).rename(body.new)
        adapter.rename(name, body.new)
        return {"ok": True}

    monkeypatch.setattr(api, "_rename_session", rename)
    pqueue.PromptQueue("velho").append("pedido anterior")
    async with asyncio.timeout(5):
        task = asyncio.create_task(api.rename_session("velho", api.RenameBody(new="novo")))
        assert await asyncio.to_thread(entered.wait, 3)
        send = asyncio.create_task(api._send_one_codex("novo", "pedido durante a renomeação"))
        await asyncio.sleep(0)
        assert not send.done()
        release.set()
        await task
        assert (await send)["ok"]
        assert [e["text"] for e in pqueue.PromptQueue("novo").load()] == [
            "pedido anterior", "pedido durante a renomeação"]
        await api.rename_session("novo", api.RenameBody(new="velho"))
        assert len(pqueue.PromptQueue("velho").load()) == 2


async def test_deliverable_libera_turno_preso_em_sessao_nao_assinada():
    # Sem assinatura nao chega turn/completed -> in_progress nunca seria limpo e TODO envio virava
    # "deferred" pra sempre, em silencio. Expira por tempo (com log) em vez de bloquear.
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1")   # subscribed=False
    assert await adapter.send_prompt("sess", "oi") == "sent"
    assert await adapter.deliverable("sess") is False      # turno recem-comecado: segura
    adapter._sessions["sess"]["in_progress_since"] -= adapter.UNSUBSCRIBED_TURN_TTL + 1
    assert await adapter.deliverable("sess") is True       # velho demais: libera


async def test_deliverable_nao_expira_turno_em_sessao_assinada():
    # Sessao assinada tem turn/completed pra limpar -> o TTL nao pode furar um turno vivo e longo.
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1", subscribed=True)
    await adapter.send_prompt("sess", "oi")
    adapter._sessions["sess"]["in_progress_since"] -= adapter.UNSUBSCRIBED_TURN_TTL * 10
    assert await adapter.deliverable("sess") is False


# --- Um consumidor de notifications por sessao (fan-out pros SSEs) ---------------------------

class _QueueClient:
    """Como o AppServerClient real: UMA fila; cada `notifications()` tira dela. Dois consumidores
    dividem os itens — e o sentinela e reposto pra nenhum deles pendurar."""

    closed = False

    def __init__(self, notifs: list[dict]):
        self._q: asyncio.Queue = asyncio.Queue()
        for n in notifs:
            self._q.put_nowait(n)
        self._q.put_nowait(None)
        self.aberturas = 0
        self.requests: list[tuple[str, dict]] = []

    async def notifications(self):
        self.aberturas += 1
        while True:
            item = await self._q.get()
            if item is None:
                self._q.put_nowait(None)
                return
            yield item
            await asyncio.sleep(0)   # da a vez ao outro consumidor, como no loop real

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        self.requests.append((method, params))
        return {}


class _LiveQueueClient:
    closed = False

    def __init__(self):
        self._q: asyncio.Queue = asyncio.Queue()
        self.aberturas = 0
        self.requests: list[tuple[str, dict]] = []

    async def notifications(self):
        self.aberturas += 1
        while (item := await self._q.get()) is not None:
            yield item

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        self.requests.append((method, params))
        return {}


async def test_bomba_continua_consumindo_sem_sse_aberto():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    adapter.attach("viva", client, "t")
    monitor = adapter.state_monitor("viva", lambda: "viva")
    await monitor.__anext__()
    await monitor.aclose()

    await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
    async with asyncio.timeout(1):
        while adapter._sessions["viva"]["state"] != "working":
            await asyncio.sleep(0)

    bomba = adapter._sessions["viva"]["bomba"]
    await client._q.put(None)
    await bomba


async def test_bomba_publica_na_fonte_recriada_depois_que_sse_fecha():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    adapter.attach("volta", client, "t")
    async with asyncio.timeout(1):
        while client.aberturas != 1:
            await asyncio.sleep(0)

    fonte_antiga = CodexPreviewSource.get("volta")
    assinatura = fonte_antiga.subscribe()
    await assinatura.__anext__()
    await assinatura.aclose()
    fonte_nova = CodexPreviewSource.get("volta")

    await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
    await client._q.put({"method": "item/agentMessage/delta",
                         "params": {"threadId": "t", "delta": "voltou"}})
    async with asyncio.timeout(1):
        while fonte_nova.text != "voltou":
            await asyncio.sleep(0)

    bomba = adapter._sessions["volta"]["bomba"]
    await client._q.put(None)
    await bomba


async def test_bomba_encerrada_reinicia_na_proxima_abertura():
    class _FalhaUmaVez(_LiveQueueClient):
        async def notifications(self):
            self.aberturas += 1
            if self.aberturas == 1:
                raise RuntimeError("falha transitória")
            while (item := await self._q.get()) is not None:
                yield item

    adapter = CodexAdapter()
    client = _FalhaUmaVez()
    adapter.attach("reinicia", client, "t")
    primeira = adapter._sessions["reinicia"]["bomba"]
    async with asyncio.timeout(1):
        while not primeira.done():
            await asyncio.sleep(0)

    stream = adapter.state_monitor("reinicia", lambda: "reinicia")
    await stream.__anext__()
    await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
    assert (await stream.__anext__()).state == "working"
    assert client.aberturas == 2
    assert "bomba_error" not in adapter._sessions["reinicia"]
    await stream.aclose()
    await client._q.put(None)
    await adapter._sessions["reinicia"]["bomba"]


async def test_dois_sse_na_mesma_sessao_recebem_a_resposta_inteira():
    # Desktop + celular no mesmo chat: cada SSE abre um state_monitor. Com um consumidor por SSE
    # os deltas eram DIVIDIDOS entre eles (cada um ficava com metade da frase) e os dois empurravam
    # buffers diferentes pro mesmo CodexPreviewSource — a previa mostrava "Faria em pequenas, o
    # atual." em vez de "Faria em mudancas pequenas, preservando o comportamento atual.".
    adapter = CodexAdapter()
    client = _QueueClient([
        {"method": "turn/started", "params": {"threadId": "t"}},
        {"method": "item/agentMessage/delta", "params": {"delta": "Faria "}},
        {"method": "item/agentMessage/delta", "params": {"delta": "em "}},
        {"method": "item/agentMessage/delta", "params": {"delta": "mudancas "}},
        {"method": "item/agentMessage/delta", "params": {"delta": "pequenas"}},
    ])
    adapter.attach("dois", client, "t")

    async def ver():
        return [ev.state async for ev in adapter.state_monitor("dois", lambda: "dois")]

    a, b = await asyncio.wait_for(asyncio.gather(ver(), ver()), timeout=5)
    assert client.aberturas == 1, "a fila do app-server tem UM consumidor por sessao"
    assert a[-1] == "working" and b[-1] == "working"
    assert CodexPreviewSource.get("dois").text == "Faria em mudancas pequenas"


async def test_ouvinte_novo_reusa_a_bomba_permanente():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    adapter.attach("janela", client, "t")
    primeiro = adapter.state_monitor("janela", lambda: "janela")
    await primeiro.__anext__()
    await primeiro.aclose()

    segundo = adapter.state_monitor("janela", lambda: "janela")
    await segundo.__anext__()
    await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
    assert (await segundo.__anext__()).state == "working"
    assert client.aberturas == 1

    await segundo.aclose()
    bomba = adapter._sessions["janela"]["bomba"]
    await client._q.put(None)
    await bomba


async def test_excecao_na_bomba_chega_no_ouvinte_em_vez_de_pendurar():
    # A bomba roda numa task propria: uma excecao la dentro nao sobe sozinha ate o pump do SSE.
    # Sem repassar, cada ouvinte ficaria em `fila.get()` pra sempre, com a tela "conectada" e muda.
    class _Quebra:
        closed = False

        async def notifications(self):
            yield {"method": "turn/started", "params": {}}
            raise RuntimeError("app-server mandou lixo")

    adapter = CodexAdapter()
    adapter.attach("quebra", _Quebra(), "t")
    with pytest.raises(RuntimeError, match="lixo"):
        async with asyncio.timeout(5):
            async for _ in adapter.state_monitor("quebra", lambda: "quebra"):
                pass


async def test_previa_zera_a_cada_agent_message_do_mesmo_turno():
    # Um turno do Codex pode ter mais de um agentMessage ("Vou conferir…" e depois a resposta).
    # O buffer colava os dois, e como o preambulo ja tinha caido no rollout (bolha propria), a
    # previa mostrava "Vou conferir.Resposta" ate o turno fechar.
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},
        {"method": "item/started", "params": {"item": {"type": "agentMessage", "text": ""}}},
        {"method": "item/agentMessage/delta", "params": {"delta": "Vou conferir."}},
        {"method": "item/completed", "params": {"item": {"type": "agentMessage",
                                                          "text": "Vou conferir."}}},
        {"method": "item/started", "params": {"item": {"type": "agentMessage", "text": ""}}},
        {"method": "item/agentMessage/delta", "params": {"delta": "Resposta"}},
    ])
    adapter.attach("itens", client, "t")
    async for _ in adapter.state_monitor("itens", lambda: "itens"):
        pass
    assert CodexPreviewSource.get("itens").text == "Resposta"


async def test_state_monitor_abre_com_o_estado_e_a_statusline_ja_conhecidos():
    # Reabrir o chat (ou abrir num 2o aparelho) no meio de um turno: a tela nascia sem estado nem
    # status line ate a PROXIMA notification do app-server — contexto e limites sumiam mesmo
    # existindo no backend. O primeiro evento e o retrato do que ja se sabe.
    adapter = CodexAdapter()
    adapter.attach("quente", _FakeClient([]), "t", model="gpt-6-astra", effort="high")
    sess = adapter._sessions["quente"]
    sess["state"] = "working"
    sess["in_progress"] = True
    sess["token_usage"] = {"last": {"inputTokens": 109_000}, "modelContextWindow": 828_000}
    events = [ev async for ev in adapter.state_monitor("quente", lambda: "quente")]
    assert events[0].state == "working"
    assert "💬" in (events[0].status_line or "")
    assert "gpt-6-astra" in events[0].status_line
