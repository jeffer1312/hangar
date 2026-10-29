"""Captura leitura, máscara e preservação de valores especiais já gravados."""
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


def observe(operation):
    try:
        return {'ok': operation()}
    except Exception as error:
        return {'error': str(error), 'kind': type(error).__name__}


cases = []
for key in ('unknown', 'editor', 'groq_api_key', 'transcription_providers'):
    for value in (float('nan'), float('inf'), '\ud800', 12, False, [], {}, ['x']):
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
            initial = json.dumps({key: value})
            runtime = config / 'runtime-config.json'
            runtime.write_text(initial)
            loaded = observe(rc._carregar)
            state = observe(rc.estado)
            applied = observe(lambda: rc.aplicar({'finish_min_seconds': 9}))
            cases.append({'key': key, 'initial': initial, 'loaded': loaded, 'state': state,
                          'applied': applied, 'disk': runtime.read_text()})

Path(__file__).with_name('runtime-special.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_runtime_special.py',
    'cases': cases,
}, indent=2, ensure_ascii=True) + '\n')
