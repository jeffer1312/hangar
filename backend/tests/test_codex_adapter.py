"""Testes do CodexAdapter: map_state (notification -> estado/statusline/preview) + orquestracao
(state_monitor/send_prompt/deliverable/transcript_stream) contra um AppServerClient FAKE (sem
spawnar o codex real — o wire protocol ja e testado em test_codex_appserver.py)."""
import json

from app.adapters.codex.adapter import CodexAdapter, map_state
from app.adapters.codex.preview import CodexPreviewSource
from app.state import StateEvent


class _FakeClient:
    """Duck-type minimo de AppServerClient: notifications() prefixadas + request() gravado."""

    def __init__(self, notifs: list[dict]):
        self._notifs = notifs
        self.requests: list[tuple[str, dict]] = []

    async def notifications(self):
        for n in self._notifs:
            yield n

    async def request(self, method: str, params: dict, timeout: float = 30.0) -> dict:
        self.requests.append((method, params))
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


def test_token_usage_statusline():
    ev = {"method": "thread/tokenUsage/updated", "params": {"threadId": "x", "turnId": "t",
          "tokenUsage": {"total": {"totalTokens": 1000}, "last": {"totalTokens": 1000},
                          "modelContextWindow": 10000}}}
    sl = map_state(ev).status_line
    assert "10%" in sl or "10" in sl  # 1000/10000


def test_agent_message_delta_is_preview():
    ev = {"method": "item/agentMessage/delta",
          "params": {"threadId": "x", "turnId": "t", "itemId": "i", "delta": "ok"}}
    r = map_state(ev)
    assert r.preview_delta == "ok"


def test_unknown_method_is_neutral():
    r = map_state({"method": "account/rateLimits/updated", "params": {}})
    assert r.state is None and r.status_line is None and r.preview_delta is None


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
    assert events[1].status_line and "50" in events[1].status_line


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


# --- CodexAdapter.transcript_stream (reaproveita TranscriptTailer + parse_rollout_line) ------

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


# --- registro em PROVIDERS -------------------------------------------------------------------

def test_codex_registered_in_providers():
    from app.adapters import PROVIDERS
    assert isinstance(PROVIDERS["codex"], CodexAdapter)
