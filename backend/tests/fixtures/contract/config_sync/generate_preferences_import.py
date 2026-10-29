"""Captura aplicação de preferências pelo importador original, incluindo efeitos e backup."""
import asyncio
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
from app import config_sync as cs, runtime_config as rc
from app.config import Settings

scenarios = [
    ({'groq_api_key': 'fixture-key'}, {'groq_api_key': rc.mascarar('fixture-key')}),
    ({'groq_api_key': 'fixture-key'}, {'groq_api_key': ''}),
    ({}, {'editor': '/invalid/editor', 'notify_finished': False}),
    ({}, {'editor': 'vim', 'upload_retention_days': float('inf')}),
    ({}, {'editor': float('nan'), 'notify_dead': False}),
    ({'extra': float('nan')}, {'editor': 'vim'}),
    ({'extra': float('inf')}, {'editor': 'vim'}),
    ({'extra': '\ud800'}, {'editor': 'vim'}),
    ({}, {'editor': '\U0010ffff'}),
    ({}, {'sync': True, 'scan_roots': '/fixture', 'term_origins': '*', 'unknown': 1}),
    ({'editor': 'vim'}, {'editor': 'vim'}),
]

async def after(*_):
    pass

cases = []
for initial, incoming in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(
        os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True
    ):
        home = Path(folder)
        claude = home / '.claude'
        claude.mkdir()
        path = claude / 'runtime-config.json'
        path.write_text(json.dumps(initial))
        mp.setattr(rc, '_backend_config_base', lambda: claude)
        mp.setattr(rc, '_cache', None)
        mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
        mp.setattr(cs, '_new_backups', lambda _: home / '.hangar/config-sync/backups/fixture')
        roots = cs.Roots(home=str(home), claude=str(claude), codex=str(home / '.codex'), hangar=str(home / 'repo'))
        bundle = cs.Bundle(items={'hangar_prefs': {'prefs': incoming}})
        report = asyncio.run(cs.apply_bundle(bundle, ['hangar_prefs'], roots, after=after))
        report = json.loads(json.dumps(report).replace(folder, '⟦HOME⟧'))
        files = {str(p.relative_to(home)).replace('\\', '/'): p.read_text()
                 for p in home.rglob('*') if p.is_file()}
        cases.append({'initial': initial, 'incoming': incoming, 'report': report, 'files': files})

Path(__file__).with_name('preferences-import.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_preferences_import.py',
    'injections': ['pós-aplicar isolado; esta captura cobre somente hangar_prefs', 'backup fixture'],
    'cases': cases,
}, indent=2, ensure_ascii=True) + '\n')
