"""Captura categoria, precedência e efeitos das falhas ao aplicar preferências."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from pytest import MonkeyPatch
from app import pensamento, runtime_config as rc
from app.config import Settings

scenarios = []
for value in [float('nan'), float('inf'), float('-inf'), 'inválido']:
    scenarios.append(('integer', {}, None, {'upload_retention_days': value}))
for value in [12, True, ['x'], ['x'] * 9, {'x': 1}, {str(i): i for i in range(9)}]:
    for incoming in ['', 'nova-chave']:
        scenarios.append(('secret-mask', {'groq_api_key': value}, None, {'groq_api_key': incoming}))
for value in [float('nan'), float('inf'), '\ud800']:
    scenarios.append(('thinking-extra', {}, json.dumps({'extra': value}), {'mostrar_pensamento': True}))
scenarios.extend([
    ('external-last', {}, None, {'mostrar_pensamento': 1, 'editor': 12}),
    ('external-last-infinite', {}, None, {'mostrar_pensamento': 1, 'upload_retention_days': float('inf')}),
    ('python-strip', {}, '\x1c{}\x1f', {'mostrar_pensamento': True, 'editor': '\x1cvim\x1f'}),
])
cases = []
for name, initial, thinking, changes in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(
        os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True
    ):
        home = Path(folder)
        config = home / '.claude'
        config.mkdir()
        mp.setattr(rc, '_backend_config_base', lambda: config)
        mp.setattr(rc, '_cache', None)
        mp.setattr(pensamento, 'compartilhado', lambda: config)
        mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
        runtime = config / 'runtime-config.json'
        runtime.write_text(json.dumps(initial))
        if thinking is not None:
            (config / 'settings.json').write_text(thinking)
        try:
            state = {'ok': rc.estado()['groq_api_key']}
        except Exception as error:
            state = {'kind': type(error).__name__, 'error': str(error)}
        try:
            result = {'ok': rc.aplicar(changes)}
        except Exception as error:
            result = {'kind': type(error).__name__, 'error': str(error)}
        cases.append({'name': name, 'initial': initial, 'thinking': thinking, 'changes': changes,
                      'state_secret': state, 'result': result, 'runtime': runtime.read_text(),
                      'settings': (config / 'settings.json').read_text() if (config / 'settings.json').exists() else None})

Path(__file__).with_name('runtime-apply-errors.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_runtime_apply_errors.py',
    'cases': cases,
}, indent=2, ensure_ascii=True) + '\n')
