"""Captura a ordem dos erros profundos sem iniciar o backend."""
import json
import subprocess
from pathlib import Path

rows = []
for depth in (900, 1500, 10000):
    for name, raw in (
        ('missing_close', '[' * depth + '0' + ']' * (depth - 1)),
        ('integer', '[' * depth + '9' * 4301 + ']' * depth),
        ('syntax_first', '[' * depth + 'x,' + '9' * 4301 + ']' * depth),
    ):
        row = {'name': name, 'depth': depth, 'raw': raw}
        try:
            json.loads(raw)
            raise AssertionError('a entrada deveria falhar')
        except json.JSONDecodeError as error:
            row.update(status=422, position=error.pos, message=error.msg)
        except ValueError:
            row.update(status=400)
        rows.append(row)
Path(__file__).with_name('json-deep-errors.json').write_text(json.dumps({
    'source': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_json_deep_errors.py',
    'rows': rows,
}, indent=2) + '\n')
