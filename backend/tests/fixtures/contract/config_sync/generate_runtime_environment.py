"""Captura precedência e duração dos valores Settings que alimentam as preferências."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
from pytest import MonkeyPatch

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from app.config import Settings
from app import runtime_config as rc

scenarios = [
    ('file', [], 'CP_EDITOR=file-editor\n'),
    ('interpolation', [('EDITOR_DEFAULT', 'from-default')], 'CP_EDITOR=${EDITOR_DEFAULT:-code}\n'),
    ('upper_lower', [('CP_EDITOR', 'upper'), ('cp_editor', 'lower')], 'CP_SHORTCUTS=${CP_EDITOR}\n'),
    ('lower_upper', [('cp_editor', 'lower'), ('CP_EDITOR', 'upper')], 'CP_SHORTCUTS=${CP_EDITOR}\n'),
    ('sdk_env_prefixed_file', [('GROQ_API_KEY', 'fixture-sdk')], 'CP_GROQ_API_KEY=fixture-file\n'),
    ('prefixed_env_sdk_file', [('CP_GROQ_API_KEY', 'fixture-env')], 'GROQ_API_KEY=fixture-file\n'),
    ('both_aliases_env', [('GROQ_API_KEY', 'fixture-sdk'), ('CP_GROQ_API_KEY', 'fixture-env')], ''),
    ('empty_prefixed_alias', [('GROQ_API_KEY', 'fixture-sdk'), ('CP_GROQ_API_KEY', '')], ''),
    ('bare_key', [], 'CP_EDITOR\n'),
]
rows = []
for name, pairs, raw in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp:
        home = Path(folder)
        config = home / '.claude'
        config.mkdir()
        dotenv = home / '.hangar/config/.env'
        dotenv.parent.mkdir(parents=True)
        dotenv.write_text(raw)
        with patch.dict(os.environ, dict([('HOME', folder), ('USERPROFILE', folder), *pairs]), clear=True):
            mp.setattr(rc, 'settings', Settings(_env_file=dotenv))
            mp.setattr(rc, '_backend_config_base', lambda: config)
            mp.setattr(rc, '_cache', None)
            def values():
                return {key: rc.get(key) for key in ('editor', 'shortcuts', 'groq_api_key')}
            before = values()
            dotenv.write_text('CP_EDITOR=late-file\nCP_GROQ_API_KEY=late-file\n')
            os.environ['CP_EDITOR'] = 'late-env'
            changed = values()
            runtime = config / 'runtime-config.json'
            runtime.write_text(json.dumps({'editor': 'runtime-editor'}))
            mp.setattr(rc, '_cache', None)
            overridden = values()
            runtime.write_text('{}')
            mp.setattr(rc, '_cache', None)
            removed = values()
        rows.append(dict(name=name, pairs=pairs, file=raw, before=before,
                         changed=changed, overridden=overridden, removed=removed))
Path(__file__).with_name('runtime-environment.json').write_text(json.dumps({
    'source': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_runtime_environment.py',
    'cases': rows,
}, ensure_ascii=False, indent=2) + '\n')
