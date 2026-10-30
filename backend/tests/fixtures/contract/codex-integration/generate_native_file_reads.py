"""Captura leitura original com arquivo ausente e links válidos ou quebrados."""
import ast
import json
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[5]
commit = '8a3ce81c1'
source = subprocess.check_output(['git', 'show', f'{commit}:backend/app/codex_arquivos.py'], cwd=root, text=True)
tree = ast.parse(source)
tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'ler']
namespace = {'Path': Path}
exec(compile(tree, 'codex_arquivos_original.py', 'exec'), namespace)
cases = []
for name in ('missing', 'regular', 'dangling', 'chain', 'valid', 'missing_parent'):
    with tempfile.TemporaryDirectory() as raw:
        home = Path(raw)
        path = home / 'ação'
        if name == 'regular':
            path.write_bytes(b'\xff\x00')
        elif name in ('dangling', 'valid'):
            path.symlink_to('target')
            if name == 'valid':
                (home / 'target').write_bytes(b'\xff\x00')
        elif name == 'chain':
            path.symlink_to('middle')
            (home / 'middle').symlink_to('absent')
        elif name == 'missing_parent':
            path = home / 'absent' / 'ação'
        try:
            value = namespace['ler'](path)
            result = {'ok': None if value is None else value.hex()}
        except Exception as error:
            result = {'class': type(error).__name__, 'error': str(error).replace(raw, '$ROOT')}
        cases.append({'name': name, 'result': result, 'symlink': path.is_symlink()})
Path(__file__).with_name('native-file-reads.json').write_text(json.dumps({
    'source_commit': commit, 'platform': 'posix',
    'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_file_reads.py',
    'cases': cases,
}, ensure_ascii=True, indent=2) + '\n')
print(len(cases))
