"""Captura o campo de contexto Codex incorporado da main durante a migração."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

backend = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(backend))
from pytest import MonkeyPatch
from app import pensamento, runtime_config as rc
from app.config import Settings

key = 'codex_extended_context'
cases = []
for env_value, initial, changes, remove in [
    (None, {}, {}, False), ('false', {}, {}, False), ('true', {}, {}, False),
    (None, {key: False}, {}, False), ('false', {key: True}, {}, False),
    (None, {}, {key: False}, False), (None, {}, {key: 'false'}, False),
    (None, {}, {key: 0}, False), (None, {}, {key: 'invalid'}, False),
    (None, {}, {key: None}, False), (None, {key: False}, {}, True),
]:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp:
        env = {'HOME': folder, 'USERPROFILE': folder}
        if env_value is not None:
            env['CP_CODEX_EXTENDED_CONTEXT'] = env_value
        with patch.dict(os.environ, env, clear=True):
            root = Path(folder) / '.claude'
            root.mkdir()
            path = root / 'runtime-config.json'
            path.write_text(json.dumps(initial))
            mp.setattr(rc, '_backend_config_base', lambda: root)
            mp.setattr(rc, '_cache', None)
            mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
            mp.setattr(pensamento, 'compartilhado', lambda: root)
            before = rc.estado()[key]
            try:
                result = {'ok': rc.aplicar(changes, remover={key} if remove else set())}
            except Exception as error:
                result = {'error': str(error)}
            cases.append(dict(env=env_value, initial=initial, changes=changes, remove=remove,
                              before=before, result=result, after=rc.estado()[key], disk=path.read_text()))

Path(__file__).with_name('codex-context.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=backend, text=True).strip(),
    'upstream_commit': 'c05445587',
    'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_codex_context.py',
    'cases': cases,
}, ensure_ascii=False, indent=2) + '\n')
print(len(cases))
