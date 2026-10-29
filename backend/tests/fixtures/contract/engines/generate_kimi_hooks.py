"""Caracteriza instalação idempotente dos hooks Kimi em diretórios temporários."""
import json
import subprocess
import tempfile
from pathlib import Path
from app import kimi_hook_installer as module
cases=[]
for initial in (None,"",'model = "pessoal"\n',"inválido",'hooks = []\n', '[[hooks]]\nevent="Stop"\ncommand="pessoal"\n'):
    with tempfile.TemporaryDirectory() as root:
        home=Path(root)
        module.kimi_home=lambda:home
        module.HOOK=str(home/"repo/backend/hooks/kimi_state_hook.py")
        module.GUARD=str(home/"repo/backend/hooks/guard_tmux.py")
        module._COMMAND=f'"{home}/repo/backend/.venv/bin/python" "{module.HOOK}"'
        module._GUARD_COMMAND=f'"{home}/repo/backend/.venv/bin/python" "{module.GUARD}"'
        # Defaults são congelados pelo Python na definição da função.
        module._blocks.__defaults__=(module._COMMAND,)
        module._missing.__defaults__=(None,module.HOOK)
        if initial is not None:(home/"config.toml").write_text(initial)
        result=module.ensure_kimi_hooks_installed()
        again=module.ensure_kimi_hooks_installed()
        files={p.name:p.read_text() for p in home.iterdir() if p.is_file()}
        cases.append(json.loads(json.dumps({"initial":initial,"expected":result,"again":again,"files":files}).replace(root,"$ROOT")))
Path(__file__).with_name("kimi-hooks.json").write_text(json.dumps({"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_kimi_hooks.py","cases":cases},ensure_ascii=False,indent=2)+"\n")
