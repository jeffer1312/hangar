"""Captura a decisão de aplicar preferências com igualdade numérica do Python."""
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

async def after(*_):
    pass

cases = []
for key, old, new in [
    ('notify_finished', 1, True), ('notify_finished', 0, False),
    ('notify_finished', True, 1), ('notify_finished', False, 0),
    ('upload_retention_days', 1, 1.0), ('upload_retention_days', 1.0, 1),
    ('editor', 0, False), ('editor', float('nan'), float('nan')),
    ('editor', float('inf'), float('inf')), ('editor', float('-inf'), float('inf')),
    ('editor', [float('nan')], [float('nan')]),
    ('editor', {'x': 1}, {'x': True}),
    ('upload_retention_days', 9007199254740993, 9007199254740992.0),
    ('upload_retention_days', 9007199254740992.0, 9007199254740993),
    ('upload_retention_days', 2**100, float(2**100)),
    ('upload_retention_days', 10**100, float(10**100)),
    ('editor', float('inf'), 10**400),
    ('editor', {'x': float('nan')}, {'x': float('nan')}),
]:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(
        os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True
    ):
        home = Path(folder)
        claude = home / '.claude'
        claude.mkdir()
        initial, incoming = {key: old}, {key: new}
        path = claude / 'runtime-config.json'
        path.write_text(json.dumps(initial))
        mp.setattr(rc, '_backend_config_base', lambda: claude)
        mp.setattr(rc, '_cache', None)
        mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
        mp.setattr(cs, '_new_backups', lambda _: home / '.hangar/config-sync/backups/fixture')
        roots = cs.Roots(home=str(home), claude=str(claude), codex=str(home / '.codex'), hangar=str(home / 'repo'))
        bundle = cs.unpack(cs.pack(cs.Bundle(items={'hangar_prefs': {'prefs': incoming}})))
        report = asyncio.run(cs.apply_bundle(bundle, ['hangar_prefs'], roots, after=after))
        report = json.loads(json.dumps(report).replace(folder, '⟦HOME⟧'))
        files = {str(p.relative_to(home)).replace('\\', '/'): p.read_text() for p in home.rglob('*') if p.is_file()}
        cases.append({'initial': initial, 'incoming': incoming, 'report': report, 'files': files})

Path(__file__).with_name('preferences-equality.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_preferences_equality.py',
    'injections': ['pós-aplicar isolado; pacote real passa por pack/unpack', 'backup fixture'],
    'cases': cases,
}, ensure_ascii=True, indent=2) + '\n')
