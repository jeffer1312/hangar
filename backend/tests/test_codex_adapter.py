"""Testes do CodexAdapter: map_state (notification -> estado/statusline/preview) + orquestracao
(send_prompt/deliverable) contra um AppServerClient FAKE (sem
spawnar o codex real — o wire protocol ja e testado em test_codex_appserver.py)."""
import json

import pytest
from unittest.mock import patch

from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex.adapter import (
    CodexAdapter, format_status_line, map_state,
)
from app.adapters.preview_push import PushPreviewSource


@pytest.fixture(autouse=True)
def _isolate_sidecar(tmp_path):
    # Sidecars duraveis redirecionados pra tmp -- mesmo padrao de test_codex_registry.py (evita
    # os testes de model/effort tocarem ~/.hangar/codex-sessions de verdade).
    with patch.object(codex_sessions, "_dir", lambda: tmp_path / "codex-sessions"), \
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
    # format_status_line.
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
    # reconhecido e o snapshot cru (primary/secondary) e capturado.
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


async def test_warm_sessions_skips_disk_when_rust_owns_every_codex(monkeypatch):
    from types import SimpleNamespace
    from app import runtime_coordinator
    adapter = CodexAdapter()
    lidos = []
    monkeypatch.setattr(codex_sessions, "list_all", lambda: lidos.append(1) or [])
    dono = {"rust": SimpleNamespace(mode="rust", rust_owns=lambda provider, headless: True),
            "parcial": SimpleNamespace(mode="rust", rust_owns=lambda provider, headless: headless)}
    monkeypatch.setattr(runtime_coordinator, "current", lambda: dono["rust"])
    await adapter.warm_sessions()
    assert lidos == []
    monkeypatch.setattr(runtime_coordinator, "current", lambda: dono["parcial"])
    await adapter.warm_sessions()
    assert lidos == [1], "Codex com terminal fora do Rust ainda precisa do laço"


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


# --- CodexAdapter.interrupt (turn/interrupt) ------------------------------------------------

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


async def test_drain_never_touches_the_queue_when_rust_owns_the_codex_terminal(monkeypatch, tmp_path):
    # A fila do Codex com terminal é a mesma projeção que o Store do Rust grava: o drain do Python
    # (SSE do convidado, `_drenar`) não pode reivindicar nem desfazer nada nela.
    from app import runtime_coordinator
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", tmp_path / "projects")
    from app.pqueue import PromptQueue
    q = PromptQueue("sess-rustterm")
    q.append("msg pendente", delivered=False)
    writes = []
    monkeypatch.setattr(PromptQueue, "claim_undelivered", lambda self, **kw: writes.append("claim") or [])
    monkeypatch.setattr(PromptQueue, "set_delivered", lambda self, *a, **kw: writes.append("set"))
    monkeypatch.setattr(runtime_coordinator, "rust_owns_codex_terminal", lambda: True)
    adapter = CodexAdapter()
    adapter.attach("sess-rustterm", _FakeClient([]), "thread-r")

    async def never(name, text):
        raise AssertionError("o Python não entrega pelo Codex com terminal do Rust")
    monkeypatch.setattr(adapter, "send_prompt", never)
    assert await adapter.drain("sess-rustterm", "") == 0
    assert writes == []
    assert [e["delivered"] for e in q.load()] == [False]


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
            "serviceTiers": [{"id": "priority", "name": "Fast", "description": "faster", "extra": True}],
            "defaultServiceTier": "priority",
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
        "serviceTiers": [{"id": "priority", "name": "Fast", "description": "faster", "extra": True}],
        "defaultServiceTier": "priority",
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
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high", "service_tier": None}


async def test_current_model_falls_back_to_sidecar_when_not_attached():
    codex_sessions.save("sess", "thread-1", "/rollout.jsonl", "/tmp/proj",
                         model="gpt-5-codex", effort="low")
    adapter = CodexAdapter()
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "low", "service_tier": None}


async def test_current_model_null_when_never_chosen():
    adapter = CodexAdapter()
    assert adapter.current_model("ghost") == {"model": None, "effort": None, "service_tier": None}


# --- modelo/effort viajam no proprio turn/start ---------------------------------------------

async def test_send_prompt_herda_configuracao_viva_sem_sobrescrever_terminal():
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    await adapter.send_prompt("sess", "oi")
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high", "service_tier": None}
    assert client.requests == [("thread/settings/update", {
        "threadId": "thread-1", "model": "gpt-5-codex", "effort": "high",
    }), ("turn/start", {
        "threadId": "thread-1", "input": [{"type": "text", "text": "oi"}],
    })]


async def test_send_prompt_omits_model_effort_when_unset():
    # Sem escolha explicita, nao manda os campos -> a thread usa o proprio default.
    adapter = CodexAdapter()
    client = _FakeClient([])
    adapter.attach("sess", client, "thread-1")
    await adapter.send_prompt("sess", "oi")
    assert client.requests == [
        ("turn/start", {"threadId": "thread-1", "input": [{"type": "text", "text": "oi"}]}),
    ]


# --- default_model (fix-model-display): display cai pro default da thread sem escolha ------

async def test_current_model_falls_back_to_default_when_no_explicit_choice():
    # attach() so com default_model (thread/start sem escolha do usuario) -> current_model
    # (usado pelo GET /models e pelo pill do front) mostra o default, nao None.
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1", default_model="gpt-5.6-sol")
    assert adapter.current_model("sess") == {"model": "gpt-5.6-sol", "effort": None, "service_tier": None}


async def test_current_model_explicit_choice_wins_over_default():
    adapter = CodexAdapter()
    adapter.attach("sess", _FakeClient([]), "thread-1", default_model="gpt-5.6-sol")
    await adapter.set_model("sess", "gpt-5-codex", "high")
    assert adapter.current_model("sess") == {"model": "gpt-5-codex", "effort": "high", "service_tier": None}


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
    assert adapter.current_model("sess") == {"model": "gpt-5.6-sol", "effort": "medium", "service_tier": None}


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


@pytest.mark.parametrize("tier", ["priority", "default"])
async def test_service_tier_waits_for_same_thread_notification_without_model_effort(tier):
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    requested = asyncio.Event()
    original = client.request

    async def request(method, params, timeout=30.0):
        result = await original(method, params, timeout)
        if method == "thread/read":
            return {"thread": {"model": "custom-model", "reasoningEffort": "high"}}
        if method == "model/list":
            return {"data": [{"model": "custom-model", "serviceTiers": [{"id": "priority", "name": "Fast"}]}]}
        if method == "thread/settings/update":
            requested.set()
        if method == "thread/resume":
            assert params == {"threadId": "t"}
            return {"thread": {"id": "t"}, "serviceTier": tier}
        return result

    client.request = request
    codex_sessions.save("sess", "t", "/rollout.jsonl", "/tmp/proj", model="custom-model", effort="high")
    adapter.attach("sess", client, "t", model="custom-model", effort="high", subscribed=True)
    listener = asyncio.Queue()
    adapter._sessions["sess"]["ouvintes"].append(listener)
    operation = asyncio.create_task(adapter.set_service_tier("sess", tier))
    try:
        await asyncio.wait_for(requested.wait(), 1)
        assert not operation.done()  # {} não confirma a aplicação.
        await client._q.put({"method": "thread/settings/updated", "params": {
            "threadId": "other", "threadSettings": {"serviceTier": tier}}})
        await client._q.put({"method": "thread/settings/updated", "params": {
            "threadId": "t", "threadSettings": {"model": "custom-model", "effort": "high"}}})
        await asyncio.wait_for(listener.get(), 1)
        assert not operation.done()
        await client._q.put({"method": "thread/settings/updated", "params": {
            "threadId": "t", "threadSettings": {"serviceTier": tier}}})
        assert await asyncio.wait_for(operation, 1) == tier
        event = await asyncio.wait_for(listener.get(), 1)
        assert event.codex_service_tier == tier
        assert adapter.current_model("sess") == {"model": "custom-model", "effort": "high", "service_tier": tier}
        saved = codex_sessions.load("sess")
        assert (saved["service_tier"], saved["model"], saved["effort"]) == (tier, "custom-model", "high")
        assert [call for call in client.requests if call[0] == "thread/settings/update"] == [
            ("thread/settings/update", {"threadId": "t", "serviceTier": tier})]
        assert client.aberturas == 1
    finally:
        operation.cancel()
        await asyncio.gather(operation, return_exceptions=True)
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


@pytest.mark.parametrize("failure", ["rpc", "timeout", "refused", "replaced"])
async def test_service_tier_unconfirmed_or_refused_never_reports_success(failure):
    adapter = CodexAdapter()
    adapter.SERVICE_TIER_TIMEOUT = 0.02
    client = _LiveQueueClient()
    original = client.request

    async def request(method, params, timeout=30.0):
        result = await original(method, params, timeout)
        if method == "thread/settings/update":
            if failure == "rpc":
                raise RuntimeError("refused")
            if failure == "refused":
                await client._q.put({"method": "thread/settings/updated", "params": {
                    "threadId": "t", "threadSettings": {"serviceTier": "priority"}}})
            if failure == "replaced":
                codex_sessions.update("sess", thread_id="new-thread")
                await client._q.put({"method": "thread/settings/updated", "params": {
                    "threadId": "t", "threadSettings": {"serviceTier": "default"}}})
        return result

    client.request = request
    codex_sessions.save("sess", "t", "/rollout.jsonl", "/tmp/proj")
    adapter.attach("sess", client, "t", subscribed=True)
    try:
        with pytest.raises(RuntimeError):
            await adapter.set_service_tier("sess", "default")
        assert adapter._sessions["sess"].get("service_tier_waiter") is None
        assert codex_sessions.load("sess").get("service_tier") != "default"
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


@pytest.mark.parametrize("blocked", ["startup", "lock"])
async def test_service_tier_deadline_includes_startup_and_lifecycle_lock(monkeypatch, blocked):
    adapter = CodexAdapter()
    adapter.SERVICE_TIER_TIMEOUT = 0.01
    client = _LiveQueueClient()
    lock = asyncio.Lock()
    adapter._locks["sess"] = lock
    adapter._sessions["sess"] = {"client": client, "thread_id": "t"}
    monkeypatch.setattr(codex_sessions, "load", lambda _name: None)

    async def ensure_running(_name):
        if blocked == "startup":
            await asyncio.Event().wait()
        return client

    monkeypatch.setattr(adapter, "ensure_running", ensure_running)
    if blocked == "lock":
        await lock.acquire()
    try:
        with pytest.raises(RuntimeError, match="prazo"):
            await asyncio.wait_for(adapter.set_service_tier("sess", "default"), 0.2)
        assert not client.requests
    finally:
        if lock.locked():
            lock.release()


async def test_priority_refuses_model_changed_while_catalog_was_loading():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    adapter.attach("sess", client, "t", model="supported", subscribed=True)

    async def request(method, params, timeout=30.0):
        if method == "thread/read":
            return {"thread": {"model": "supported"}}
        if method == "model/list":
            adapter._sessions["sess"]["model"] = "unsupported"
            return {"data": [{"model": "supported", "serviceTiers": [{"id": "priority"}]}]}
        pytest.fail("alteração enviada para modelo sem suporte")

    client.request = request
    try:
        with pytest.raises(RuntimeError):
            await adapter.set_service_tier("sess", "priority")
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


async def test_priority_uses_live_catalog_and_default_can_disable_unsupported_model():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    advertised = []
    effective = None
    original = client.request

    async def request(method, params, timeout=30.0):
        nonlocal effective
        result = await original(method, params, timeout)
        if method == "thread/read":
            return {"thread": {"model": "custom", "reasoningEffort": "high"}}
        if method == "model/list":
            return {"data": [{"model": "custom", "serviceTiers": list(advertised)}]}
        if method == "thread/settings/update":
            effective = params["serviceTier"]
            await client._q.put({"method": "thread/settings/updated", "params": {
                "threadId": "t", "threadSettings": {"serviceTier": effective}}})
        if method == "thread/resume":
            return {"thread": {"id": "t"}, "serviceTier": effective}
        return result

    client.request = request
    adapter.attach("sess", client, "t", model="custom", subscribed=True)
    try:
        with pytest.raises(RuntimeError):
            await adapter.set_service_tier("sess", "priority")
        assert not any(method == "thread/settings/update" for method, _ in client.requests)
        advertised.append({"id": "priority", "name": "Fast"})
        assert await adapter.set_service_tier("sess", "priority") == "priority"
        advertised.clear()
        assert await adapter.set_service_tier("sess", "default") == "default"
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


async def test_subscription_old_snapshot_does_not_overwrite_new_service_tier():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    codex_sessions.save("sess", "t", "/rollout.jsonl", "/tmp/proj")
    adapter.attach("sess", client, "t", model="old")
    listener = asyncio.Queue()
    sess = adapter._sessions["sess"]
    sess["ouvintes"].append(listener)

    async def request(method, params, timeout=30.0):
        assert method == "thread/resume"
        assert "serviceTier" not in params and "model" not in params and "effort" not in params
        await client._q.put({"method": "thread/settings/updated", "params": {
            "threadId": "t", "threadSettings": {"model": "new", "effort": "high", "serviceTier": "priority"}}})
        await listener.get()
        return {"thread": {"id": "t"}, "model": "old", "reasoningEffort": "low", "serviceTier": "default"}

    client.request = request
    try:
        await adapter._subscribe_when_ready("sess", "/tmp/proj")
        assert adapter.current_model("sess") == {"model": "new", "effort": "high", "service_tier": "priority"}
        assert codex_sessions.load("sess")["service_tier"] == "priority"
    finally:
        await client._q.put(None)
        await sess["bomba"]


async def test_subscription_recovers_external_service_tier_and_persists_it():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    codex_sessions.save("sess", "t", "/rollout.jsonl", "/tmp/proj", service_tier="default")
    adapter.attach("sess", client, "t")

    async def request(method, params, timeout=30.0):
        return {"thread": {"id": "t", "serviceTier": "default"}, "serviceTier": "priority"}

    client.request = request
    try:
        await adapter._subscribe_when_ready("sess", "/tmp/proj")
        assert adapter.current_model("sess")["service_tier"] == "priority"
        assert codex_sessions.load("sess")["service_tier"] == "priority"
        assert (await adapter.read_settings("sess"))["service_tier"] == "priority"
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


@pytest.mark.parametrize("before_ack", [False, True])
async def test_service_tier_stale_candidates_need_effective_snapshot_and_capture_during_read(before_ack):
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    updated = asyncio.Event()
    ack = asyncio.Event()
    reading = asyncio.Event()
    resume_reply = asyncio.get_running_loop().create_future()
    snapshots = 0
    adapter.attach("sess", client, "t", subscribed=True)
    sess = adapter._sessions["sess"]
    listener = asyncio.Queue()
    sess["ouvintes"].append(listener)

    async def request(method, params, timeout=30.0):
        nonlocal snapshots
        client.requests.append((method, params))
        if method == "thread/settings/update":
            updated.set()
            await ack.wait()
            return {}
        assert method == "thread/resume" and params == {"threadId": "t"}
        snapshots += 1
        if snapshots == 1:
            reading.set()
            return await resume_reply
        return {"thread": {"id": "t"}, "serviceTier": "default"}

    async def notify(tier):
        await client._q.put({"method": "thread/settings/updated", "params": {
            "threadId": "t", "threadSettings": {"serviceTier": tier}}})
        await asyncio.wait_for(listener.get(), 1)

    client.request = request
    operation = asyncio.create_task(adapter.set_service_tier("sess", "default"))
    try:
        await asyncio.wait_for(updated.wait(), 1)
        await notify("priority")
        assert not operation.done()
        if before_ack:
            await notify("default")
            assert snapshots == 0
        ack.set()
        if not before_ack:
            await notify("default")
        await asyncio.wait_for(reading.wait(), 1)
        assert not operation.done()
        await notify("default")  # A próxima candidata não pode se perder durante a consulta.
        resume_reply.set_result({"thread": {"id": "t"}, "serviceTier": "priority"})
        assert await asyncio.wait_for(operation, 1) == "default"
        assert snapshots == 2
        assert len([call for call in client.requests if call[0] == "thread/settings/update"]) == 1
    finally:
        operation.cancel()
        await asyncio.gather(operation, return_exceptions=True)
        await client._q.put(None)
        await sess["bomba"]


async def test_service_tier_null_means_default_in_notification_and_resume():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    adapter.attach("sess", client, "t", subscribed=True)

    async def request(method, params, timeout=30.0):
        if method == "thread/settings/update":
            await client._q.put({"method": "thread/settings/updated", "params": {
                "threadId": "t", "threadSettings": {"serviceTier": None}}})
            return {}
        assert method == "thread/resume"
        return {"thread": {"id": "t"}, "serviceTier": None}

    client.request = request
    try:
        assert await adapter.set_service_tier("sess", "default") == "default"
        assert adapter.current_model("sess")["service_tier"] == "default"
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


@pytest.mark.parametrize("tier", ["priority", "default"])
async def test_service_tier_already_active_returns_without_waiting(tier):
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    codex_sessions.save("sess", "t", "/rollout.jsonl", "/tmp/proj", service_tier=tier)
    adapter.attach("sess", client, "t", service_tier=tier, subscribed=True)
    try:
        assert await asyncio.wait_for(adapter.set_service_tier("sess", tier), 1) == tier
        assert client.requests == []
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


async def test_settings_notification_keeps_model_when_sidecar_thread_changed():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    codex_sessions.save("sess", "other-thread", "/rollout.jsonl", "/tmp/proj", service_tier="default")
    adapter.attach("sess", client, "t", model="old", service_tier="default", subscribed=True)
    listener = asyncio.Queue()
    adapter._sessions["sess"]["ouvintes"].append(listener)
    try:
        await client._q.put({"method": "thread/settings/updated", "params": {
            "threadId": "t", "threadSettings": {"serviceTier": "priority", "model": "new"}}})
        await asyncio.wait_for(listener.get(), 1)
        sess = adapter._sessions["sess"]
        assert (sess["model"], sess["service_tier"], sess["settings_revision"]) == ("new", "default", 1)
        assert codex_sessions.load("sess")["service_tier"] == "default"
    finally:
        await client._q.put(None)
        await adapter._sessions["sess"]["bomba"]


async def test_bomba_publica_na_fonte_recriada_depois_que_sse_fecha():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    adapter.attach("volta", client, "t")
    async with asyncio.timeout(1):
        while client.aberturas != 1:
            await asyncio.sleep(0)

    fonte_antiga = PushPreviewSource.get("volta")
    assinatura = fonte_antiga.subscribe()
    await assinatura.__anext__()
    await assinatura.aclose()
    fonte_nova = PushPreviewSource.get("volta")

    await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
    await client._q.put({"method": "item/agentMessage/delta",
                         "params": {"threadId": "t", "delta": "voltou"}})
    async with asyncio.timeout(1):
        while fonte_nova.text != "voltou":
            await asyncio.sleep(0)

    bomba = adapter._sessions["volta"]["bomba"]
    await client._q.put(None)
    await bomba


async def _wait_preview_text(name: str, expected: str) -> None:
    async with asyncio.timeout(2):
        while PushPreviewSource.get(name).text != expected:
            await asyncio.sleep(0)


async def _finish_preview_client(adapter: CodexAdapter, name: str, client) -> None:
    task = adapter._sessions.get(name, {}).get("bomba")
    await client._q.put(None)
    if task is not None:
        await task


async def test_coalesced_preview_does_not_join_distinct_agent_messages():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-item-clear"
    adapter.attach(name, client, "t")
    try:
        await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "Preamble"}})
        await _wait_preview_text(name, "Preamble")
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-stale"}})
        await client._q.put({"method": "item/completed", "params": {"item": {"type": "agentMessage", "text": "Preamble-stale"}}})
        await client._q.put({"method": "item/started", "params": {"item": {"type": "agentMessage", "text": ""}}})
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "Reply"}})
        await _wait_preview_text(name, "Reply")
        await asyncio.sleep(0.2)
        assert PushPreviewSource.get(name).text == "Reply"
    finally:
        await _finish_preview_client(adapter, name, client)


async def test_final_pending_prefix_reaches_recreated_source_after_last_sse_closes():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-source-recreate"
    adapter.attach(name, client, "t")
    old = PushPreviewSource.get(name)
    subscription = old.subscribe()
    await subscription.__anext__()
    try:
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "first"}})
        await _wait_preview_text(name, "first")
        await subscription.aclose()
        assert name not in PushPreviewSource._sources
        current = PushPreviewSource.get(name)
        assert current is not old
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-last"}})
        await _wait_preview_text(name, "first-last")
        assert old.text == "first"
        assert current.text == "first-last"
    finally:
        await subscription.aclose()
        await _finish_preview_client(adapter, name, client)


async def test_replaced_session_generation_cannot_publish_old_pending_prefix():
    adapter = CodexAdapter()
    old_client = _LiveQueueClient()
    new_client = _LiveQueueClient()
    name = "preview-generation"
    adapter.attach(name, old_client, "old-thread")
    await old_client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "old"}})
    await _wait_preview_text(name, "old")
    await old_client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-pending"}})
    await asyncio.sleep(0)
    old_task = adapter._sessions[name]["bomba"]
    adapter.attach(name, new_client, "new-thread")
    try:
        await new_client._q.put({"method": "turn/started", "params": {"threadId": "new-thread"}})
        await new_client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "new"}})
        await _wait_preview_text(name, "new")
        await asyncio.sleep(0.2)
        assert PushPreviewSource.get(name).text == "new"
    finally:
        await _finish_preview_client(adapter, name, new_client)
        await asyncio.gather(old_task, return_exceptions=True)


async def test_close_preserve_preview_keeps_last_published_text_without_late_timer(monkeypatch):
    monkeypatch.setattr(codex_adapter, "matar_app_server", lambda name: None)
    monkeypatch.setattr(codex_adapter.sem_terminal, "matar", lambda meta: None)
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-preserve-close"
    adapter.attach(name, client, "t")
    await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "first"}})
    await _wait_preview_text(name, "first")
    await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-pending"}})
    await asyncio.sleep(0)
    task = adapter._sessions[name]["bomba"]
    adapter.close_sync(name, preserve_preview=True)
    await asyncio.gather(task, return_exceptions=True)
    await asyncio.sleep(0.2)
    assert PushPreviewSource.get(name).text == "first"


@pytest.mark.parametrize("failure", [ConnectionError, TimeoutError])
async def test_drain_never_replays_prompt_when_rpc_confirmation_is_lost(monkeypatch, tmp_path, failure):
    from app import pqueue

    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("uncertain-codex")
    entry = queue.append("mensagem recebida sem confirmação", delivered=False)

    class Client(_FakeClient):
        async def request(self, method, params, timeout=30):
            self.requests.append((method, params))
            raise failure("a resposta do envio não chegou")

    client = Client([])
    adapter = CodexAdapter()
    adapter.attach("uncertain-codex", client, "thread")
    assert await adapter.drain("uncertain-codex", "") == 0
    assert await adapter.drain("uncertain-codex", "") == 0
    assert len(client.requests) == 1
    rows = queue.load()
    assert rows[0]["id"] == entry["id"]
    assert rows[0]["delivered"] is True
    assert not rows[0].get("confirmed")


async def test_pane_morto_nao_apaga_o_sidecar_nem_o_servidor_de_outra_vida(tmp_path):
    # S-25: o nome recriado no segundo em que o vigia ainda olhava o pane velho é outra vida; o
    # sidecar e o app-server dela ficam.
    adapter = CodexAdapter()
    codex_sessions.save("cx", "t-a", "", str(tmp_path), endpoint="ws://127.0.0.1:1", app_pid=111)
    adapter._sessions["cx"] = {"client": _FakeClient([])}
    mortos = []
    with patch.object(codex_adapter.tmux, "sessao_existe", return_value=False), \
         patch.object(codex_adapter, "matar_app_server", mortos.append):
        vigia = asyncio.create_task(adapter._watch_tmux("cx"))
        await asyncio.sleep(0)
        codex_sessions.save("cx", "t-b", "", str(tmp_path), endpoint="ws://127.0.0.1:2", app_pid=222)
        await vigia
    assert (codex_sessions.load("cx") or {}).get("app_pid") == 222
    assert mortos == []
