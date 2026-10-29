"""Congela o limite de inteiros do leitor JSON antes da validação dos modelos."""
import json
import subprocess
from pathlib import Path

large = '9' * 4301
small = '9' * 4300
inputs = [
    ('boundary', small), ('overflow', large), ('negative_boundary', '-' + small),
    ('negative_overflow', '-' + large), ('float', large + '.0'),
    ('exponent', large + 'e0'), ('quoted', json.dumps(large)),
    ('leading_zero', '0' + large), ('after_value', '0 ' + large),
    ('before_trailing_comma', '{"x":' + large + ',}'),
    ('missing_colon', '{"x" ' + large + '}'),
    ('after_nonfinite', '[NaN,' + large + ',]'),
    ('syntax_after_nonfinite', '[NaN bad,' + large + ']'),
    ('after_surrogate', '{"x":"\\ud800","i":' + large + '}'),
    ('after_invalid_escape', '{"x":"\\q","i":' + large + '}'),
    ('object_key', '{' + large + ':0}'),
    ('array_missing_comma', '[0 ' + large + ']'),
    ('negative_infinity', '[-Infinity,' + large + ']'),
    ('invalid_unicode_escape', '{"x":"\\u1😀","i":' + large + '}'),
    ('array_leading_zero', '[0' + large + ']'),
    ('object_leading_zero', '{"x":0' + large + '}'),
    ('nested_boundary', '[' * 150 + small + ']' * 150),
    ('nested_overflow', '[' * 150 + large + ']' * 150),
]
rows = []
for name, raw in inputs:
    row = {'name': name, 'raw': raw}
    try:
        row['value_json'] = json.dumps(json.loads(raw), ensure_ascii=True)
    except json.JSONDecodeError as error:
        row.update(status=422, message=error.msg, position=error.pos)
    except ValueError:
        row.update(status=400)
    rows.append(row)
Path(__file__).with_name('json-numbers.json').write_text(json.dumps({
    'source': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_json_numbers.py',
    'cases': rows,
}, ensure_ascii=True, indent=2) + '\n')
