"""Congela todos os bytes da serialização profunda por tamanho e SHA-256."""
import hashlib
import json
import subprocess
from pathlib import Path

rows = []
for depth, indent in ((10000, None), (900, 2)):
    for shape, opening, closing in (('array', '[', ']'), ('object', '{"x":', '}'),
                                    ('ordered', '{"z":0,"x":', '}')):
        for sort_keys in ([False, True] if indent is None else [False]):
            raw = opening * depth + '0' + closing * depth
            output = json.dumps(json.loads(raw), ensure_ascii=False, indent=indent,
                                sort_keys=sort_keys).encode()
            rows.append({'shape': shape, 'depth': depth, 'indent': indent, 'sort_keys': sort_keys,
                         'bytes': len(output), 'sha256': hashlib.sha256(output).hexdigest()})
        if indent is None:
            output = json.dumps(json.loads(raw), ensure_ascii=False, separators=(',', ':')).encode()
            rows.append({'shape': shape, 'depth': depth, 'indent': None, 'sort_keys': False,
                         'compact': True, 'bytes': len(output),
                         'sha256': hashlib.sha256(output).hexdigest()})
raw = '{"ação":["maçã",true,null,{"x":1.0}],"escape":"\\n\\t\\u0000"}'
output = json.dumps(json.loads(raw), ensure_ascii=False, separators=(',', ':')).encode()
rows.append({'input': raw, 'compact': True, 'bytes': len(output),
             'sha256': hashlib.sha256(output).hexdigest()})
Path(__file__).with_name('json-deep-output.json').write_text(json.dumps({
    'source': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_json_deep_output.py',
    'rows': rows,
}, indent=2) + '\n')
