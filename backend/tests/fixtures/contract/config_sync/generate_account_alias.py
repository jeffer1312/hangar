"""Congela admissão e reaplicação de apelidos pelo consumidor config-sync original."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from pytest import MonkeyPatch
from app import apelidos, config_sync as sync
from app.config_sync_paths import Roots

cases = []
for alias in ('\x1c', '\x1cNome\x1f', '  ', 'Nome', '𝄞' * 42, 'x' * 39 + ' fim'):
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp:
        home = Path(folder)
        path = home / '.claude-fixture'
        path.mkdir()
        target = home / 'aliases.json'
        mp.setattr(sync.contas, 'caminho', lambda name: path)
        mp.setattr(sync.contas, 'e_conta', lambda path: True)
        mp.setattr(apelidos, '_caminho', lambda: target)
        key = sync._alias_id(path)
        target.write_text(json.dumps({key: 'Original'}))
        ctx = SimpleNamespace(roots=Roots(str(home), str(home / '.claude'),
                                         str(home / '.codex'), str(home)))
        steps = []
        for _ in range(2):
            result = {'changed': [], 'warnings': []}
            sync._apply_account(ctx, result, 'fixture', {'alias': alias, 'settings': {}})
            steps.append({'result': result, 'alias': apelidos.ler().get(key),
                          'disk': target.read_text().replace(folder, '<HOME>')})
        cases.append({'alias': alias, 'steps': steps})

Path(__file__).with_name('account-alias.json').write_text(json.dumps({
    'source': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_account_alias.py',
    'cases': cases,
}, indent=2, ensure_ascii=True) + '\n')
