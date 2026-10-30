"""Captura a saída mantida por descendente depois de o líder administrativo sair."""
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
program = '''
import json, os, sys, time
scenario = sys.argv[1]
assert sys.argv[2:4] == ['-c', 'project_root_markers=[]']
if os.fork():
    sys.exit(0)
time.sleep(0.05 if scenario == 'descendant' else 30)
print(json.dumps({'descendant': True}))
'''
namespace['PROGRAM'] = program


async def main():
    cases = [await namespace['capture'](scenario, 'cli', scenario)
             for scenario in ('descendant', 'descendant-timeout')]
    Path(__file__).with_name('native-cli-descendants.json').write_text(json.dumps({
        'source_commit': namespace['COMMIT'], 'platform': 'posix',
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_cli_descendants.py',
        'boundary': 'Executável controlado faz fork real; CLI, communicate e limpeza originais.',
        'program': program, 'cases': cases,
    }, ensure_ascii=True, indent=2) + '\n')
    print(len(cases))


asyncio.run(main())
