"""Caracteriza preferências persistidas incompatíveis com nomes de sessão."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch)
    from app import push
    push.settings.projects_dir=Path(scratch)/'projects'
    values=[None,False,1,'sessao',{}, {'s':1}, [], ['s','outra','s'], [1,2], [None], [False,1,0], [100000000000000000001,100000000000000000000], ['s',1], [[]], [{}]]
    rows=[]
    for value in values:
        for muted in [False,True]:
            initial={'muted':value,'extra':'preservar'}
            path=push._prefs_file();path.write_text(json.dumps(initial),encoding='utf-8');before=path.read_bytes()
            try: decision={'value':push.is_muted('s')}
            except Exception as error:decision={'error':type(error).__name__}
            try:push.set_muted('s',muted);result={'ok':True}
            except Exception as error:result={'error':type(error).__name__}
            rows.append({'muted':value,'requested':muted,'decision':decision,'write':result,'unchanged':path.read_bytes()==before,'after':path.read_text()})
    out={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_muted.py','isolation':'is_muted e set_muted originais, arquivo temporário; sem autenticação, avisos ou rede.','rows':rows}
    Path(__file__).with_name('push-muted.json').write_text(json.dumps(out,ensure_ascii=True,indent=2)+'\n')
    print(len(rows))
