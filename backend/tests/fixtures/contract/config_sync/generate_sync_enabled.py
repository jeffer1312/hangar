"""Captura a leitura efetiva do estado de sincronização nas preferências persistidas."""
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
from app import runtime_config as rc, sync
from app.config import Settings

values = [False, True, 0, 1, -1, '', 'false', [], [1], {}, {'x': 1}, None,
          float('nan'), float('inf')]
documents = [json.dumps({'sync': value}) for value in values]
documents += ['{}', 'null', '[]', '{', json.dumps({'sync': True, 'extra': float('nan')})]
cases = []
for default in [False, True]:
    for document in documents:
        with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(
            os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True
        ):
            path = Path(folder) / 'runtime-config.json'
            path.write_text(document)
            mp.setattr(rc, '_backend_config_base', lambda: Path(folder))
            mp.setattr(rc, '_cache', None)
            mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots='', sync=default))
            cases.append({'default': default, 'document': document, 'response': sync._setup_status(None)})

Path(__file__).with_name('sync-enabled.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_sync_enabled.py',
    'cases': cases,
}, ensure_ascii=True, indent=2) + '\n')
