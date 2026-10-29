"""Captura janela e relógio do limitador original sem autenticação nem rede."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    from app import sync
    rows=[]
    cases=[('boundary',1,300,[1000,1299.9,1300]),('fractional',1,1,[1000.9,1001.1,1001.9]),
           ('zero_max',0,300,[1000,1001]),('negative_max',-1,300,[1000,1001]),
           ('huge_max',10**80,300,[1000,1000,1000]),('zero_window',1,0,[1000,1000,999]),
           ('negative_window',1,-1,[1000,1000,999,998]),('huge_window',1,10**80,[1000,2000]),
           ('clock_back',1,300,[1000,999,1300]),('separate_ips',1,300,[1000,1000,1000])]
    for name,maximum,window,times in cases:
        sync._FAILS.clear();steps=[]
        with patch.object(sync.settings,'sync_rate_max',maximum),patch.object(sync.settings,'sync_rate_window',window):
            for index,now in enumerate(times):
                ip='192.0.2.2' if name=='separate_ips' and index==1 else '192.0.2.1'
                with patch('app.sync.time.time',lambda:now):
                    limited=sync.rate_limited(ip)
                    if not limited:sync.record_fail(ip)
                steps.append({'now':now,'ip':ip,'limited':limited,'hits':json.loads(json.dumps(sync._FAILS))})
        rows.append({'name':name,'maximum':maximum,'window':window,'steps':steps})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_rate.py','rows':rows}
    (HERE/'rate.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} cenários do limitador capturados')
