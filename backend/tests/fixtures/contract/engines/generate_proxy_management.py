"""Captura a chave administrativa no diretório compartilhado, com conta alternativa."""
import json
import os
import subprocess
import tempfile
from pathlib import Path
from app import cliproxy
with tempfile.TemporaryDirectory() as directory:
    home = Path(directory)
    os.environ.update(HOME=directory, USERPROFILE=directory, CLAUDE_CONFIG_DIR=str(home / "alternative"))
    (home / ".claude").mkdir()
    cliproxy.set_management_key(" fixture-management ")
    path = cliproxy._mgmt_path()
    result = {"path": str(path.relative_to(home)), "read": cliproxy.management_key(), "content": path.read_text(), "mode": path.stat().st_mode & 0o777}
    cliproxy.set_management_key("")
    result["removed"] = not path.exists()
    result["empty"] = cliproxy.management_key()
Path(__file__).with_name("proxy-management.json").write_text(json.dumps({"source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), "command": "PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_proxy_management.py", "expected":result}, ensure_ascii=False, indent=2)+"\n")
