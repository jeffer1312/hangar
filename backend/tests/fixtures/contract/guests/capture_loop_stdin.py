"""Executa o lançador original com um executável descartável que só registra stdin."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
sys.path.insert(0,str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='fixture')
    from app.loop import _rodar_claude_p
    target=root/'stdin.bin';exe=root/'fixture-cli'
    exe.write_text('#!/bin/sh\ncat > "'+str(target)+'"\nprintf "fixture\\n"\n');exe.chmod(0o700)
    rows=[]
    for raw in ['"a\\ud800b"','"\\udfff"','"\\ud83d\\ude00"','"\\ufffd"','"texto\\u0000fim"','"ação"']:
        text=json.loads(raw)
        result=_rodar_claude_p(str(exe),text,None)
        rows.append({'raw':raw,'stdin_hex':target.read_bytes().hex(),'returncode':result.returncode,'stdout':result.stdout})
    payload={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_stdin.py','isolation':'_rodar_claude_p original e subprocesso temporário sem LLM; captura Linux UTF-8 errors=replace; nenhum backend real','rows':rows}
    (Path(__file__).resolve().parent/'loop-stdin.json').write_text(json.dumps(payload,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} entradas stdin capturadas')
