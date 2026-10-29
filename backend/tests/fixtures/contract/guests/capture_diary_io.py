"""Captura erros SO do cofre original, com falha injetada na fronteira de arquivo."""
import json
import logging
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CP_SYNC_DATA=str(Path(scratch)/'vault.json'), CP_AUTH_TOKEN='fixture')
    from app import sync, diag
    logging.disable(logging.CRITICAL)
    rows=[]
    for operation in ['load','save']:
        for number in [13,28,21]:
            events=[]
            def record(evento,nivel='ok',**fields):
                events.append({'evento':evento,'nivel':nivel,**{k:v for k,v in fields.items() if k!='ms' and v is not None}})
            with patch.object(diag,'registrar',record), patch('pathlib.Path.read_text' if operation=='load' else 'tempfile.mkstemp',side_effect=OSError(number,'conteúdo privado','arquivo-privado')):
                try:
                    with diag.operacao('sync.prova'):
                        sync.load_vault() if operation=='load' else sync.save_vault({})
                except OSError:
                    pass
            rows.append({'operation':operation,'os_code':number,'events':events})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_diary_io.py',
            'isolation':'sync.load_vault/save_vault e diag.operacao originais; OSError injetado em read_text/mkstemp, sem HTTP; errno POSIX nesta captura', 'rows':rows}
    (HERE/'diary-io.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} erros SO capturados')
