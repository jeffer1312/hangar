"""Captura fusão de hooks e escritas com comparação antes da troca."""
import json
import subprocess
from pathlib import Path
from app.codex_arquivos import mesclar_hooks

def group(command,matcher="Bash"):
    return {"matcher":matcher,"hooks":[{"type":"command","command":command}]}
documents=[{}, {"hooks":{}}, {"description":"pessoal","hooks":{"PreToolUse":[group("antigo"),group("exclusivo","Write")]}}, {"hooks":{"PreToolUse":[group("novo"),{"matcher":"Bash","hooks":[]}]}}, {"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"prompt","prompt":"pessoal"},*group("antigo")["hooks"]]}]}}, {"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":""}]}]}}, {"hooks":[]}, {"hooks":{"PreToolUse":"errado"}}]
cases=[]
for current in documents:
    for source in documents[:5]:
        for previous in documents[:3]:
            case={"current":current,"source":source,"previous":previous}
            try:case["expected"]={"ok":mesclar_hooks(current,source,previous)}
            except ValueError as error:case["expected"]={"error":str(error)}
            cases.append(case)
Path(__file__).with_name("codex-files.json").write_text(json.dumps({"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_codex_files.py","cases":cases},ensure_ascii=False,indent=2)+"\n")
