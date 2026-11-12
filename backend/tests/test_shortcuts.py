import json

import pytest

from app import runtime_config as rc


@pytest.fixture(autouse=True)
def _isolate(tmp_path, monkeypatch):
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    yield


def _as_json(*items) -> str:
    return json.dumps(list(items))


# --- validação na gravação -------------------------------------------------------------------

def test_valid_list_is_accepted_and_empty_restores_default():
    value = _as_json(
        {"id": "terminal", "type": "internal", "action": "terminal"},
        {"id": "a1", "type": "send_text", "label": "Relatório", "text": "/relatorio-pm",
         "icon": "emoji:📋", "send_direct": True},
        {"id": "a2", "type": "shell", "label": "Editor", "command": "code .", "confirm": True},
    )
    rc.aplicar({"shortcuts": value})
    assert rc.get("shortcuts") == value
    rc.aplicar({}, remover={"shortcuts"})
    assert rc.get("shortcuts") == ""


def test_broken_json_is_rejected():
    with pytest.raises(ValueError, match="JSON invalido"):
        rc.aplicar({"shortcuts": "{nao é json"})


def test_unknown_type_is_rejected_naming_the_item():
    with pytest.raises(ValueError, match="item 2"):
        rc.aplicar({"shortcuts": _as_json(
            {"id": "terminal", "type": "internal", "action": "terminal"},
            {"id": "x", "type": "foguete"},
        )})


def test_internal_with_unknown_action_is_rejected():
    with pytest.raises(ValueError, match="action desconhecida"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "internal", "action": "jetpack"})})


def test_item_missing_required_field_is_rejected():
    with pytest.raises(ValueError, match="sem texto"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "send_text", "label": "Oi"})})
    with pytest.raises(ValueError, match="sem comando"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "shell", "label": "Oi", "command": " "})})
    with pytest.raises(ValueError, match="sem rotulo"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "shell", "command": "true"})})
    with pytest.raises(ValueError, match="sem id"):
        rc.aplicar({"shortcuts": _as_json({"type": "internal", "action": "rodar"})})


def test_value_must_be_a_list():
    with pytest.raises(ValueError, match="esperado uma lista"):
        rc.aplicar({"shortcuts": json.dumps({"id": "x"})})


def test_shell_only_fields_are_validated():
    shell = {"id": "x", "type": "shell", "label": "L", "command": "c", "runs_in": "hangar",
             "hangar_home": False, "answer_in_app": False}
    rc.validate_shortcut_item(shell, "w")
    rc.validate_shortcut_item({**shell, "runs_in": "session"}, "w")
    with pytest.raises(ValueError, match="runs_in"):
        rc.validate_shortcut_item({**shell, "runs_in": "global"}, "w")
    with pytest.raises(ValueError, match="hangar_home"):
        rc.validate_shortcut_item({**shell, "hangar_home": "sim"}, "w")
    with pytest.raises(ValueError, match="answer_in_app"):
        rc.validate_shortcut_item({**shell, "answer_in_app": 1}, "w")
    with pytest.raises(ValueError, match="runs_in"):
        rc.validate_shortcut_item(
            {"id": "y", "type": "send_text", "label": "L", "text": "t", "runs_in": "hangar"}, "w")


