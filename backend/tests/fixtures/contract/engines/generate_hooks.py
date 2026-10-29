"""Captura a atualização de hooks sem ler configurações pessoais."""
import copy
import json
import subprocess
from pathlib import Path
from app import hook_installer

cases=[]
command='"python" "/fixture/state_hook.py" || exit 0'
entry={"type":"command","command":command}
for event in ("PreToolUse","SessionStart"):
    for initial in ({},{"hooks":{}},{"hooks":{event:[]}}, {"hooks":{event:"estranho"}}, {"model":"pessoal","hooks":{event:[{"matcher":"Bash","hooks":[{"type":"command","command":"terceiro"},entry,entry]}]}}, {"hooks":{event:[{"hooks":[{"command":"python /antigo/state_hook.py"}]},{"hooks":[entry]}]}}):
        for replacement in (command,None):
            data=copy.deepcopy(initial)
            changed=hook_installer._sync_hook(data,event,replacement,matcher="Bash",por_nome=True,script="state_hook.py")
            cases.append({"initial":initial,"event":event,"command":replacement,"expected":data,"changed":changed})
Path(__file__).with_name("hooks.json").write_text(json.dumps({"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_hooks.py","cases":cases},ensure_ascii=False,indent=2)+"\n")
