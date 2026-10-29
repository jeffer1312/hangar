"""Unicode legítimo não pertence ao espaço de marcadores do transcript."""
import json
from pathlib import Path
import subprocess

values = [chr(point) for point in (0x10F7FF, 0x10F800, 0x10F801, 0x10FFFE, 0x10FFFF)]
values.extend([
    ''.join(chr(point) for point in range(0x10F800, 0x110000)),
    '\U0010fffforq:fixture:surrogate:d800;',
    {'\U0010ffff': ['\ud800', '\U0010f800', float('nan'), float('inf')]},
    {'\U0010f800': 1, '\ud800': 2, '\U0010f7ff': 3},
])
Path(__file__).with_name('json-literal-unicode.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_json_literal_unicode.py',
    'cases': [{'input': json.dumps(value), 'ordinary': json.dumps(value),
               'sorted': json.dumps(value, sort_keys=True)} for value in values],
}, indent=2, ensure_ascii=True) + '\n')
