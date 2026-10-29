"""Erros posteriores a escalares que o json.loads aceita e o serde não representa."""
import json
import subprocess
from pathlib import Path

rows = []
for token in ('NaN', 'Infinity', '-Infinity', '"\\ud800"', '1e400'):
    for pattern in ('[{},]', '{{"x":{},}}', '[{} bad]', '[{},{{"x":}}]', '{} x',
                    '[{} 0]', '1{}', '{}0', '{{{}:0}}'):
        raw = pattern.format(token)
        row = {'raw': raw}
        try:
            row['value_json'] = json.dumps(json.loads(raw), ensure_ascii=True)
        except json.JSONDecodeError as error:
            row.update(status=422, message=error.msg, position=error.pos)
        rows.append(row)
Path(__file__).with_name('json-diagnostic-special.json').write_text(json.dumps({
    'source': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_json_diagnostic_special.py',
    'cases': rows,
}, indent=2, ensure_ascii=True) + '\n')
