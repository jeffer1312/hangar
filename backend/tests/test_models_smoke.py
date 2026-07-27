# backend/tests/test_models_smoke.py
from app.config import settings
from app.models import SessionInfo, ChatEvent, StateEvent


def test_settings_defaults():
    assert settings.port == 8765


def test_models_construct():
    assert SessionInfo(name="cc").state == "idle"
    assert ChatEvent(kind="user_msg", id="1", text="hi").text == "hi"
    assert StateEvent(session="cc", state="working", label="Elucidating…").label == "Elucidating…"
    assert StateEvent(session="cc", state="awaiting_input", options=["Yes", "No"]).options == ["Yes", "No"]


def test_chat_event_scrubs_lone_surrogates_anywhere():
    # Rede na fronteira do contrato: o parser normaliza o TEXTO, mas tool_input/id vem crus do
    # transcript (qualquer provider). Um surrogate solto ali derrubava o /history e o SSE inteiros.
    import json

    ev = ChatEvent(kind="tool_use", id="n\ud83d1", tool_name="Write", tool_use_id="c\udc4d1",
                   tool_input={"path": "/tmp/a", "content": "corte \ud83d"})
    assert ev.id == "n�1" and ev.tool_use_id == "c�1"
    assert ev.tool_input["content"] == "corte �"
    assert json.loads(ev.model_dump_json())["tool_input"]["content"] == "corte �"


def test_chat_event_leaves_wellformed_text_alone():
    ev = ChatEvent(kind="user_msg", id="1", text="tudo certo 🇧🇷 👨‍👩‍👧‍👦 ✅")
    assert ev.text == "tudo certo 🇧🇷 👨‍👩‍👧‍👦 ✅"
