"""Testes do CodexAdapter: map_state (notification -> estado/statusline/preview) + orquestracao
(state_monitor/send_prompt/deliverable/transcript_stream) contra um AppServerClient FAKE (sem
spawnar o codex real — o wire protocol ja e testado em test_codex_appserver.py)."""
import json

import pytest
from unittest.mock import patch

from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex.adapter import CodexAdapter, format_status_line, map_state
from app.adapters.codex.preview import CodexPreviewSource
from app.state import StateEvent


@pytest.fixture(autouse=True)
def _isolate_sidecar(tmp_path):
    # Sidecars duraveis redirecionados pra tmp -- mesmo padrao de test_codex_registry.py (evita
    # os testes de model/effort tocarem ~/.claude-pocket/codex-sessions de verdade).
    with patch.object(codex_sessions, "_dir", lambda: tmp_path / "codex-sessions"):
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
    token_usage = {"total": {"totalTokens": 14389},
                    "last": {"inputTokens": 0, "outputTokens": 0, "totalTokens": 14389},
                    "modelContextWindow": 258400}
    rate_limits = {"primary": {"usedPercent": 0, "windowDurationMins": 10080,
                    "resetsAt": now + 6 * 86400}}
    sl = format_status_line("GPT-5.5", "high", token_usage, rate_limits, now=now)
    assert sl == "🤖 GPT-5.5 (high) │ 💬 0/0 14k/258k │ 📅7d:0% ↺6d"


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
    assert [e.state for e in events] == ["working", "idle"]


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
    assert [e.state for e in events] == ["idle"]


async def test_state_monitor_carries_status_line_without_changing_state():
    adapter = CodexAdapter()
    client = _FakeClient([
        {"method": "turn/started", "params": {}},
        {"method": "thread/tokenUsage/updated", "params": {
            "tokenUsage": {"total": {"totalTokens": 5000}, "modelContextWindow": 10000}}},
    ])
    adapter.attach("sess", client, "t")
    events = [ev async for ev in adapter.state_monitor("sess", lambda: "sess")]
    assert [e.state for e in events] == ["working", "working"]  # segue o ultimo estado conhecido
    # sem model/effort escolhidos (attach sem eles) -> so a secao de contexto aparece.
    assert events[1].status_line == "💬 0/0 5k/10k"


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
    assert last.status_line.startswith("🤖 gpt-5.5 (high) │ 💬 10/5 1k/10k │ 📅7d:20%")


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
    assert [e.state for e in events] == ["working", "idle"]  # StateEvents intactos (nao regrediu)
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
    assert [e.state for e in events] == ["working"]
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


async def test_send_prompt_calls_turn_start():
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    result = await adapter.send_prompt("sess", "oi")
    assert result == "sent"
    assert client.requests == [(
        "turn/start",
        {"threadId": "thread-1", "input": [{"type": "text", "text": "oi", "text_elements": []}]},
    )]


async def test_send_prompt_marks_in_progress_on_sent():
    # Fix 2: send_prompt tem que setar in_progress=True ao entregar -- senao deliverable() continua
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
    # send_prompt captura o turnId do turno recem-iniciado; interrupt manda turn/interrupt com
    # threadId+turnId (shape confirmado no schema TurnInterruptParams da 0.141.0).
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.send_prompt("sess", "oi")
    assert await adapter.interrupt("sess") is True
    assert ("turn/interrupt", {"threadId": "thread-1", "turnId": "turn-fake"}) in client.requests


async def test_interrupt_noop_when_not_attached():
    adapter = CodexAdapter()
    assert await adapter.interrupt("ghost") is False


async def test_interrupt_noop_when_no_turn_in_flight():
    # anexada mas sem turno em voo (nunca deu send_prompt) -> nada a interromper, no-op seguro.
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    assert await adapter.interrupt("sess") is False
    assert client.requests == []


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
    assert ("account/rateLimits/read", {}) in client.requests


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


# --- send_prompt inclui model/effort no turn/start (Task C) ---------------------------------

async def test_send_prompt_includes_model_and_effort_when_set():
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    await adapter.send_prompt("sess", "oi")
    assert client.requests == [(
        "turn/start",
        {"threadId": "thread-1", "input": [{"type": "text", "text": "oi", "text_elements": []}],
         "model": "gpt-5-codex", "effort": "high"},
    )]


async def test_send_prompt_omits_model_effort_when_unset():
    # sem escolha -> turn/start nao carrega model/effort, app-server usa o default da thread
    # (mesmo shape do teste original test_send_prompt_calls_turn_start -- nao regride).
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.send_prompt("sess", "oi")
    assert client.requests == [(
        "turn/start",
        {"threadId": "thread-1", "input": [{"type": "text", "text": "oi", "text_elements": []}]},
    )]


async def test_ensure_running_resume_restores_model_effort_from_sidecar():
    # Pos-restart: dict vazio, sidecar tem a escolha gravada antes do restart. ensure_running
    # (resume) tem que repovoar model/effort no dict quente -- senao o proximo turn/start
    # "esqueceria" a escolha ate o usuario reabrir o picker.
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj",
                         model="gpt-5-codex", effort="high")
    adapter = CodexAdapter()
    client = _FakeClient([])
    with patch("app.adapters.codex.adapter.AppServerClient", lambda *a, **k: client):
        await adapter.ensure_running("sess")
    assert adapter._sessions["sess"]["model"] == "gpt-5-codex"
    assert adapter._sessions["sess"]["effort"] == "high"
    await adapter.send_prompt("sess", "oi")
    start_params = next(p for m, p in client.requests if m == "turn/start")
    assert start_params["model"] == "gpt-5-codex"
    assert start_params["effort"] == "high"


def test_transcript_stream_creates_rollout_dir(tmp_path):
    # 1a sessao Codex do dia: o dir do rollout (~/.codex/sessions/YYYY/MM/DD) ainda nao existe quando o
    # SSE abre o tail -> sem o mkdir, awatch(parent) em follow() derruba o SSE com FileNotFoundError.
    from app.adapters.codex.adapter import CodexAdapter
    rollout = tmp_path / "2026" / "07" / "14" / "rollout-x.jsonl"
    assert not rollout.parent.exists()
    CodexAdapter().transcript_stream(str(rollout))   # a chamada sync ja roda o mkdir (antes do return)
    assert rollout.parent.exists()
