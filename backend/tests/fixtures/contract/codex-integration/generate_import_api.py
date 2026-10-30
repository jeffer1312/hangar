"""Captura validações e conclusões antecipadas do importador original."""
import ast
import asyncio
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
source = subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_importador.py'], cwd=ROOT, text=True)
tree = ast.parse(source)
error = next(node for node in tree.body if isinstance(node, ast.ClassDef) and node.name == 'CodexNativoErro')
native = next(node for node in tree.body if isinstance(node, ast.ClassDef) and node.name == 'CodexNativo')
methods = {'detectar', 'historicos_importacao', 'importar'}
native.body = [node for node in native.body if isinstance(node, ast.AsyncFunctionDef) and node.name in methods]
namespace = {'asyncio': asyncio}
exec(compile(ast.Module(body=[error, native], type_ignores=[]), 'codex_importador_original.py', 'exec'), namespace)


async def capture(operation, response, events=None, delayed=False):
    obj = namespace['CodexNativo']()
    obj.timeout = 0.02
    obj._import_lock = asyncio.Lock()
    obj._completions = None
    calls = []

    async def request(method, params):
        calls.append({'method': method, 'params': params})
        if events is not None:
            queue = obj._completions
            for event in events:
                if delayed:
                    asyncio.get_running_loop().call_later(0.001, queue.put_nowait, event)
                else:
                    queue.put_nowait(event)
        return response

    obj.request = request
    try:
        result = await getattr(obj, operation)(*[[]] if operation == 'importar' else [])
        result = {'ok': result}
    except Exception as exc:
        result = {'error': str(exc)}
    return {'operation': operation, 'response': response, 'events': events, 'delayed': delayed,
            'result': result, 'calls': calls, 'completion_cleared': obj._completions is None}


async def main():
    rows = []
    for operation, field in [('detectar', 'items'), ('historicos_importacao', 'data')]:
        for value in [None, [], [{}], [1], {}, 'x']:
            rows.append(await capture(operation, {field: value}))
        rows.append(await capture(operation, {}))
    for field in ['sourceErrors', 'errors', 'warnings']:
        for value in [[], ['falha'], {}, {'x': 1}, '', 'falha', False, True]:
            rows.append(await capture('detectar', {'items': [], field: value}))
    completion = {'importId': 'one', 'itemTypeResults': [{'failures': [{'message': 'Falha parcial'}]}]}
    for delayed in [False, True]:
        rows.append(await capture('importar', {'importId': 'one'}, [{'importId': 'other'}, completion], delayed))
    for response in [{}, {'importId': ''}, {'importId': 1}, {'importId': None}]:
        rows.append(await capture('importar', response, []))
    for events in [[], [None], [{'importId': 'one'}], [{'importId': 'one', 'itemTypeResults': {}}], [{'importId': 'one', 'itemTypeResults': []}]]:
        rows.append(await capture('importar', {'importId': 'one'}, events))
    path = Path(__file__).with_name('import-api.json')
    path.write_text(json.dumps({'source_commit': COMMIT,
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_import_api.py',
        'cases': rows}, ensure_ascii=True, indent=2) + '\n')
    print(len(rows))


asyncio.run(main())
