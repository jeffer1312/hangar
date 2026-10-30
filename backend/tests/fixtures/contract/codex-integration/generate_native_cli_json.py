"""Captura a semântica JSON do subprocesso administrativo original."""
import ast
import asyncio
import json
from pathlib import Path

source = Path(__file__).with_name('generate_native_cli.py')
tree = ast.parse(source.read_text())
tree.body = [node for node in tree.body if not (
    isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)
    and isinstance(node.value.func, ast.Attribute) and node.value.func.attr == 'run')]
namespace = {'__file__': str(source)}
exec(compile(tree, str(source), 'exec'), namespace)
program = r'''
import json, sys
scenario = sys.argv[1]
assert sys.argv[2:4] == ['-c', 'project_root_markers=[]']
if scenario == 'deep':
    value = 1
    for _ in range(200):
        value = [value]
    print(json.dumps({'value': value}))
elif scenario == 'nonfinite':
    print('{"nan":NaN,"positive":Infinity,"negative":-Infinity}')
elif scenario == 'surrogate':
    print(r'{"value":"\ud800"}')
elif scenario == 'utf16':
    sys.stdout.buffer.write('{"value":"ação"}'.encode('utf-16'))
elif scenario == 'bom':
    sys.stdout.buffer.write(b'\xef\xbb\xbf{"value":1}')
elif scenario == 'empty':
    pass
elif scenario == 'whitespace':
    print('   ')
else:
    print('{"value":1234567890123456789012345678901234567890}')
'''
namespace['PROGRAM'] = program


async def main():
    cases = [await namespace['capture'](scenario, 'cli', scenario)
             for scenario in ('deep', 'nonfinite', 'surrogate', 'utf16', 'bom', 'empty', 'whitespace', 'wide')]
    Path(__file__).with_name('native-cli-json.json').write_text(json.dumps({
        'source_commit': namespace['COMMIT'], 'platform': 'posix',
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_cli_json.py',
        'boundary': 'Bytes de stdout controlados; parser, diagnóstico e limpeza originais.',
        'program': program, 'cases': cases,
    }, ensure_ascii=True, indent=2) + '\n')
    print(len(cases))


asyncio.run(main())
