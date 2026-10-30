"""Captura leitura real e truthiness das preferências persistidas, sem iniciar serviços."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
for key in list(os.environ):
    if key.upper().startswith('CP_'):
        del os.environ[key]
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    os.chdir(scratch)
    from app import config, runtime_config
    path = Path(scratch) / 'runtime-config.json'
    runtime_config._caminho = lambda: path
    runtime_config.settings = SimpleNamespace(automations=True, notify_dead=True)
    rows = []
    for key in ['automations', 'notify_dead']:
        for value in [None, False, True, 0, 1, '', 'false', [], [1], {}, {'x': 1}, float('nan'), float('inf')]:
            path.write_text(json.dumps({key: value}))
            runtime_config._cache = None
            enabled = config.automations_enabled() if key == 'automations' else bool(runtime_config.get(key))
            rows.append({'key': key, 'value': value, 'enabled': enabled})
    data = {'source_commit': subprocess.check_output(['git', '-C', str(HERE), 'rev-parse', 'HEAD'], text=True).strip(),
            'command': 'uv run --directory backend python tests/fixtures/contract/guests/capture_automation_flags.py',
            'isolation': 'get/_carregar reais sobre arquivo temporário; defaults injetados. automations_enabled real; notify_dead testa a expressão bool do consumidor, sem executar sweep ou entrega.',
            'rows': rows}
    (HERE / 'automation-flags.json').write_text(json.dumps(data, ensure_ascii=True, indent=2) + '\n')
    print(f'{len(rows)} casos capturados')
