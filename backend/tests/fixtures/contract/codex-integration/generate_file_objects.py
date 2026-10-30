"""Captura o leitor/escritor JSON e as permissões de backup originais, em pasta isolada."""
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
source = subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_arquivos.py'], cwd=ROOT, text=True)
namespace = dict(Path=Path, hashlib=hashlib, json=json, os=os)
names = {'ler', 'json_obj', 'json_bytes', 'backup'}
nodes = [node for node in ast.parse(source).body if isinstance(node, ast.FunctionDef) and node.name in names]
exec(compile(ast.Module(body=nodes, type_ignores=[]), 'codex_arquivos.py', 'exec'), namespace)
cases = []
inputs = [('missing', None), ('unicode', '{"texto":"ação"}'.encode()),
          ('nonfinite', b'{"a":NaN,"b":Infinity,"c":-Infinity}'),
          ('wide', ('{"n":' + '9' * 100 + '}').encode()),
          ('surrogate', b'{"s":"\\ud800"}'), ('array', b'[]'), ('invalid', b'{')]
for encoding in ['utf-8-sig', 'utf-16', 'utf-16-be', 'utf-32', 'utf-32-be']:
    inputs.append((encoding, '{"texto":"ação"}'.encode(encoding)))
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / 'object.json'
    for name, raw in inputs:
        path.unlink(missing_ok=True)
        if raw is not None:
            path.write_bytes(raw)
        row = dict(name=name, raw=raw.hex() if raw is not None else None)
        try:
            value = namespace['json_obj'](path)
            row['value'] = json.dumps(value, ensure_ascii=True)
            try:
                row['output'] = namespace['json_bytes'](value).hex()
            except Exception as error:
                row['write_error'] = type(error).__name__
        except Exception as error:
            row['read_error'] = type(error).__name__
        cases.append(row)
    modes = []
    for exists in [False, True]:
        root = Path(directory) / str(exists)
        if exists:
            root.mkdir(mode=0o755)
            root.chmod(0o755)
        namespace['backup'](path, b'original', root)
        modes.append(dict(existing=exists, directory_mode=root.stat().st_mode & 0o777,
                          file_mode=next(root.iterdir()).stat().st_mode & 0o777))
Path(__file__).with_name('file-objects.json').write_text(json.dumps(dict(source_commit=COMMIT,
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_file_objects.py',
    cases=cases, modes=modes), ensure_ascii=True, indent=2) + '\n')
print(len(cases), len(modes))
