"""Captura resolução de imports sem expandir variáveis nem interpretar aspas."""
import ast
import json
import os
import pwd
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
source = subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_instrucoes.py'], cwd=ROOT, text=True)
node = next(node for node in ast.parse(source).body if isinstance(node, ast.FunctionDef) and node.name == '_importado')
namespace = dict(Path=Path, os=os)
exec(compile(ast.Module(body=[node], type_ignores=[]), 'codex_instrucoes_original.py', 'exec'), namespace)
cases = []
lines = ['@extra.md', ' @extra.md\n', '@extra name.md', '@ extra.md', '@\u0085extra.md',
         '@', '', 'extra.md', '@~/home.md', '@~\\extra.md', '@$HOME/extra.md',
         '@"extra.md"', '@absent.md', '@directory', '@./extra.md', '@../home.md',
         '\u001c@extra.md\u001f', '@extra.md trailing', '@~usuario-inexistente-hangar/extra.md']
with tempfile.TemporaryDirectory() as raw:
    home = Path(raw)
    base = home / 'project'
    base.mkdir()
    (base / 'directory').mkdir()
    files = ['home.md', 'project/extra.md', 'project/extra name.md', 'project/~\\extra.md',
             'project/$HOME/extra.md', 'project/"extra.md"']
    for name in files:
        path = home / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('Importado\n')
    previous = os.environ.get('HOME')
    os.environ['HOME'] = raw
    try:
        for line in lines + ['@' + str(base / 'extra.md'), '@/' + str(base / 'extra.md'),
                             '@//' + str(base / 'extra.md'), '@directory/../extra.md']:
            result = namespace['_importado'](line, base)
            cases.append({'line': line.replace(raw, '$ROOT'),
                          'result': None if result is None else str(result).replace(raw, '$ROOT')})
        account = pwd.getpwuid(os.getuid())
        relative = os.path.relpath(home / 'home.md', account.pw_dir)
        result = namespace['_importado'](f'@~{account.pw_name}/{relative}', base)
        cases.append({'line': '$NAMED_IMPORT', 'result': None if result is None else
                      str(result).replace(account.pw_dir, '$ACCOUNT_HOME').replace(relative, '$RELATIVE')})
    finally:
        if previous is None:
            del os.environ['HOME']
        else:
            os.environ['HOME'] = previous
Path(__file__).with_name('instruction-imports.json').write_text(json.dumps({
    'source_commit': COMMIT,
    'platform': os.name,
    'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_instruction_imports.py',
    'files': files, 'cases': cases}, ensure_ascii=True, indent=2) + '\n')
print(len(cases))
