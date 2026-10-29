"""Documenta falhas silenciosas originais; a API checked Rust as torna explícitas."""
import json
import logging
import subprocess
import tempfile
from pathlib import Path
from unittest.mock import patch
from app import codex_hook_installer, kimi_hook_installer
logging.disable(logging.CRITICAL)
cases = []
for provider, filename in [("kimi", "config.toml"), ("codex", "hooks.json")]:
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder)
        path = home / filename
        path.write_text("{ inválido", encoding="utf-8")
        if provider == "kimi":
            with patch.object(kimi_hook_installer, "kimi_home", return_value=home):
                result = kimi_hook_installer.ensure_kimi_hooks_installed()
        else:
            result = codex_hook_installer.ensure_codex_state_hook_installed(home)
        cases.append({"provider": provider, "initial": "{ inválido", "result": result,
                      "after": path.read_text(encoding="utf-8")})
Path(__file__).with_name("hook-failures.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "command": "PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_hook_failures.py",
    "cases": cases}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
