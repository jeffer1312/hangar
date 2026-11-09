import json


_SHELL = {"id": "p1", "type": "shell", "label": "Debug", "command": "true", "pasta": "sub"}


def test_global_shortcuts_still_ignore_pasta():
    # `pasta` so vale por projeto: o validador global segue como era.
    from app import runtime_config as rc
    rc._validate_shortcuts(json.dumps([{**_SHELL, "pasta": 3}]))
