"""Captura a verdade dos campos antigos e a adoção de vida ao consultar a lista."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch)
    from app import guest_users as store
    store._path_override=Path(scratch)/'guests.json'
    rows=[]
    for flag in [None,False,True,0,1,'','false',[],[1],{}, {'x':1}]:
        before={'guests':[{'id':'guest','name':'Ana','root':scratch,'token_hash':'hash','owner_sees':flag}],
                'sessions':{'s0':{'guest':'guest','life':None}}}
        store._path_override.write_text(json.dumps(before));store._reset()
        with patch('app.guest_users.session_life',lambda _:None):
            names=store.filter_visible(None,['s0','s1'],lambda name:name)
        rows.append({'before':before,'names':names})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_visibility.py','rows':rows}
    (HERE/'visibility.json').write_text(json.dumps(result,ensure_ascii=False,indent=2).replace(scratch,'⟦ROOT⟧')+'\n')
    print(f'{len(rows)} casos de visibilidade capturados')
