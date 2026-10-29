"""Captura o reconhecimento original em POSIX e Windows sem alterar o sistema operacional."""
import json
import ntpath
import posixpath
import subprocess
import types
from pathlib import Path
from app import hook_installer as module
cases = []
original = module.os
for windows in (False, True):
    module.os = types.SimpleNamespace(name="nt" if windows else "posix", path=ntpath if windows else posixpath)
    for command, script in [
        (r"python /opt/Hangar\ App/backend/hooks/kimi_state_hook.py", "/opt/Hangar App/backend/hooks/kimi_state_hook.py"),
        ("python '/broken quote /opt/kimi_state_hook.py", "/opt/kimi_state_hook.py"),
        (r'python "C:\Hangar App\hooks\state_hook.py" || exit 0', r"C:\Hangar App\hooks\state_hook.py"),
        (r"python C:\Hangar\hooks\state_hook.py", r"C:\Hangar\hooks\state_hook.py"),
        (r'python "/opt/Hangar\ App/state_hook.py"', r"/opt/Hangar\ App/state_hook.py"),
        ("python /opt/hook.py\\", "/opt/hook.py"),
    ]:
        for basename in (False, True):
            cases.append(dict(windows=windows, command=command, script=script, basename=basename, expected=module._refers_to(command, script, por_nome=basename)))
module.os = original
Path(__file__).with_name("hook-tokens.json").write_text(json.dumps({"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_hook_tokens.py","cases":cases},ensure_ascii=False,indent=2)+"\n")
