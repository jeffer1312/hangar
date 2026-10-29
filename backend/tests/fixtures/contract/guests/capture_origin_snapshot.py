"""Captura o snapshot de Settings aplicado à Origin do terminal original."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch,patch.dict(os.environ,{'HOME':scratch,'USERPROFILE':scratch},clear=True):
    from app import termsock,runtime_config
    from app.config import Settings
    envfile=Path(scratch)/'.env'
    envfile.write_text('CP_PUBLIC_URL=https://public.example:8443\nCP_TERM_ORIGINS=https://extra.example\n')
    snapshot=Settings(_env_file=envfile)
    origins=['https://public.example:8443','https://extra.example','https://changed.example','https://new-extra.example']
    rows=[]
    with patch.object(termsock,'settings',snapshot),patch.object(termsock,'_peers_conhecidos',lambda:[]),patch.object(runtime_config,'get',lambda key:''):
        for phase in ['before','after']:
            if phase=='after':
                envfile.write_text('CP_PUBLIC_URL=https://changed.example\nCP_TERM_ORIGINS=https://new-extra.example\n')
                os.environ.update(CP_PUBLIC_URL='https://changed.example',CP_TERM_ORIGINS='https://new-extra.example')
            rows.extend({'phase':phase,'origin':origin,'accepted':termsock._origem_aceita(origin,'fixture')} for origin in origins)
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_origin_snapshot.py','rows':rows}
    (HERE/'origin-snapshot.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(json.dumps(rows))
