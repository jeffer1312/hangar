"""Captura gravação, espera pela vida e troca de identidade sem multiplexador real."""
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
    initial={'guests':[{'id':'guest','name':'Ana','root':scratch,'token_hash':'hash'}], 'sessions':{}}
    rows=[]
    for sequence in [[None,None,None],[None,None,'k:third'],['k:first']]:
        store._path_override.write_text(json.dumps(initial));store._reset()
        with patch('app.guest_users.session_life',side_effect=sequence) as life,patch('app.guest_users.time.sleep') as sleep:
            store.claim('session','guest')
        rows.append({'before':initial,'sequence':sequence,'calls':life.call_count,'sleeps':[v.args[0] for v in sleep.call_args_list],
                     'disk':json.loads(store._path_override.read_text())})
    changes=[]
    for present in [False,True]:
        for value in [None,'k:new','']:
            before=initial|{'sessions':{'session':{'guest':'guest','life':'k:old'}} if present else {}}
            store._path_override.write_text(json.dumps(before));store._reset();store.set_life('session',value)
            changes.append({'before':before,'life':value,'disk':json.loads(store._path_override.read_text())})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_claim.py','claims':rows,'changes':changes}
    (HERE/'claim.json').write_text(json.dumps(result,ensure_ascii=False,indent=2).replace(scratch,'⟦ROOT⟧')+'\n')
    print(f'{len(rows)} gravações e {len(changes)} trocas de vida capturadas')
