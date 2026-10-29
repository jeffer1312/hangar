"""Captura a instalação dos hooks próprios, sem aprovação ou configuração real."""
import json
import subprocess
import tempfile
from pathlib import Path
from app import codex_hook_installer as module
cases=[]
for windows in (False,True):
    for initial in (None,"", "inválido",'[]',json.dumps({"hooks":{"Stop":[{"hooks":[{"type":"command","command":'"other-python" "/other/state_hook.py" || exit 0'}]}]}}),json.dumps({"hooks":{"Stop":"pessoal"}})):
        with tempfile.TemporaryDirectory() as root:
            home=Path(root)
            module._PYTHON=str(home/"repo/backend/.venv/bin/python")
            for name,file in (("STATE_HOOK","state_hook.py"),("NAV_HOOK","nav_hook.py"),("GUARD_HOOK","guard_tmux.py")):
                setattr(module,name,str(home/"repo/backend/hooks"/file))
            module._STATE_COMMAND=f'"{module._PYTHON}" "{module.STATE_HOOK}" || exit 0'
            module._NAV_COMMAND=f'"{module._PYTHON}" "{module.NAV_HOOK}" || exit 0'
            module._tem_hook.__defaults__=(module.STATE_HOOK,)
            module._reescrever_formato_cmd.__defaults__=(module.STATE_HOOK,)
            if initial is not None:(home/"hooks.json").write_text(initial)
            result=module.ensure_codex_state_hook_installed(home,windows=windows)
            again=module.ensure_codex_state_hook_installed(home,windows=windows)
            content=(home/"hooks.json").read_text() if (home/"hooks.json").exists() else None
            cases.append(json.loads(json.dumps({"windows":windows,"initial":initial,"expected":result,"again":again,"content":content}).replace(root,"$ROOT")))
Path(__file__).with_name("codex-hooks.json").write_text(json.dumps({"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_codex_hooks.py","cases":cases},ensure_ascii=False,indent=2)+"\n")
