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
