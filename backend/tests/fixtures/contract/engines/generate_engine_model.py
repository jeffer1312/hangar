"""Captura o driver original de modelo arbitrário sem tmux nem configuração pessoal."""
import itertools
import json
import subprocess
from pathlib import Path
from unittest.mock import patch
from app import terminal_input, model_picker

fixture = Path(__file__).parents[2]
idle = "assistant: pronto\n❯ \n"
picker = (fixture / "pane_model_picker_fable.txt").read_text(encoding="utf-8")
old = "❯ \n  ⎿  Set model to k3-256k and saved as your default for new sessions\n"
new = old + "  ⎿  Set model to k3 and saved as your default for new sessions\n"
cases = []
for name, model, panes in [
    ("success", "k3", [idle, new]),
    ("trim", " k3 ", [idle, new]),
    ("old_confirmation", "k3", [idle, old, old, new]),
    ("rejected_picker", "missing", [idle, picker]),
    ("unconfirmed", "k3", [idle]),
    ("space", "k3 --argument", [idle]),
    ("control", "k3\x00", [idle]),
    ("empty", "", [idle]),
    ("unicode_space", "k3\u00a0bad", [idle]),
]:
    queue = list(panes)
    def capture(_name):
        return queue.pop(0) if len(queue) > 1 else queue[0]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=capture), \
         patch.object(terminal_input, "send_keys") as sent, \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"), \
         patch.object(terminal_input.time, "monotonic", side_effect=itertools.count(step=0.5)):
        try:
            result = {"ok": terminal_input.TerminalInput().set_engine_model("fixture", model)}
        except model_picker.PickerError as error:
            result = {"error": {"status": error.status, "detail": error.detail}}
        except ValueError as error:
            result = {"invalid": str(error)}
        cases.append({"name": name, "model": model, "panes": panes,
                      "keys": [call.args[1] for call in sent.call_args_list], "expected": result})
Path(__file__).with_name("engine-model.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "command": "PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_engine_model.py",
    "cases": cases}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
