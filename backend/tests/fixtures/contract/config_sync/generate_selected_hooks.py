"""Captura os sete instaladores individualmente, em uma conta descartável."""
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
from contextlib import ExitStack

from app import hook_installer as hooks, pair, rust_bins

NAMES = ["askq_hook", "state_hooks", "preview_hook", "subagent_hook", "pair_hook", "nav_hook", "guard_hooks"]
cases = []
for name in NAMES:
    for initial in [{}, {"model": "pessoal", "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "pessoal"}]}]}}, {"hooks": None}]:
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            account = root / ".claude"
            account.mkdir()
            settings = account / "settings.json"
            settings.write_text(json.dumps(initial), encoding="utf-8")
            stack.enter_context(patch.object(hooks, "list_config_dirs", return_value=[SimpleNamespace(path=str(account))]))
            stack.enter_context(patch.object(hooks, "_backend_config_base", return_value=account))
            stack.enter_context(patch.object(rust_bins, "find_bin", return_value="/fixture/hangar-server"))
            stack.enter_context(patch.object(pair, "_pair_dir", return_value=account / ".hangar-pair"))
            result = getattr(hooks, f"ensure_{name}_installed")()
            def normalize(value):
                if isinstance(value, str):
                    return value.replace(hooks._PYTHON, "/fixture/python").replace(str(hooks._HOOKS_DIR), "/fixture/repo/backend/hooks").replace(directory, "/fixture/home")
                if isinstance(value, list):
                    return [normalize(item) for item in value]
                if isinstance(value, dict):
                    return {key: normalize(item) for key, item in value.items()}
                return value
            after = json.loads(settings.read_text(encoding="utf-8"))
            second = getattr(hooks, f"ensure_{name}_installed")()
            cases.append({"installer": f"ensure_{name}_installed", "initial": initial,
                          "after": normalize(after), "touched": normalize(result), "second": second,
                          "guard_link": (account / "hooks/guard_tmux.py").is_symlink()})
Path(__file__).with_name("selected-hooks.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "command": "cd backend && PYTHONPATH=. uv run python tests/fixtures/contract/config_sync/generate_selected_hooks.py",
    "normalization": "Caminhos do Python, hooks e conta substituídos por /fixture; binário pair-context injetado.",
    "cases": cases}, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
