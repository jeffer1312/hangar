"""Captura a conversão int das preferências, distinta dos modelos Pydantic."""
import json
from pathlib import Path
import subprocess
import sys

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from app import runtime_config as rc

values = [None, True, False, [], {}, 0, -1, 1.9, -0.9, -1.9, 1e30,
          float('nan'), float('inf'), float('-inf'), '', ' ', '0', '+12', '-0', '-1',
          '1_2', '_12', '12_', '1__2', '1.0', '1e2', '0x12', '\x1c12\x1f',
          '\u00a012\u2003', '\u0661\u0662', '\uff11\uff12', '1\u0662', '\u00b2',
          '\u216b', '+\u0661_\u0662', '\u200b12', '9' * 100, '\ud800',
          '9' * 4300, '9' * 4301, '0' * 4301, '_'.join('1' * 4301)]
cases = []
for key, kind in rc.EDITAVEIS.items():
    if kind is not int:
        continue
    for value in values:
        try:
            result = {'ok': rc._coagir(key, value)}
        except Exception as error:
            result = {'kind': type(error).__name__, 'error': str(error)}
        cases.append({'key': key, 'input': value, 'result': result})

Path(__file__).with_name('integer-preferences.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_integer_preferences.py',
    'cases': cases,
}, ensure_ascii=True, indent=2) + '\n')
