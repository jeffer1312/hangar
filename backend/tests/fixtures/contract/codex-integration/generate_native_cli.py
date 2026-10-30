"""Captura CLI administrativa original com subprocessos locais e limpeza real."""
import ast
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'


def original(path):
    return subprocess.check_output(['git', 'show', f'{COMMIT}:{path}'], cwd=ROOT, text=True)


namespace = {}
tree = ast.parse(original('backend/app/codex_importador.py'))
tree.body = [node for node in tree.body if not (isinstance(node, ast.ImportFrom) and (node.module or '').startswith('app'))]
exec(compile(tree, 'codex_importador_original.py', 'exec'), namespace)
lifecycle = next(node for node in ast.parse(original('backend/app/account_lifecycle.py')).body
                 if isinstance(node, ast.AsyncFunctionDef) and node.name == 'complete_on_cancel')
exec(compile(ast.Module(body=[lifecycle], type_ignores=[]), 'account_lifecycle_original.py', 'exec'), namespace)
namespace.update(substituir=os.replace, account_lifecycle=SimpleNamespace(complete_on_cancel=namespace['complete_on_cancel']),
                 diag=SimpleNamespace(conta_id=lambda path: 'fixture'))

PROGRAM = r'''
import json, os, sys, time
scenario = sys.argv[1]
assert sys.argv[2:4] == ['-c', 'project_root_markers=[]']
if scenario in ('timeout', 'cancel', 'close'):
    time.sleep(30)
if scenario == 'exit':
    print('fixture-secret', file=sys.stderr)
    sys.exit(7)
if scenario == 'auto':
    print('auto-upgrade was in flight', file=sys.stderr)
    sys.exit(1)
if scenario == 'invalid':
    print('not json')
elif scenario == 'array':
    print('[]')
elif scenario == 'errors':
    print(json.dumps({'errors': ['fixture-error']}))
elif scenario == 'inventory':
    print(json.dumps({'installed': [{'pluginId': 'a@m', 'version': '1'}]}))
elif scenario == 'bad_inventory':
    print(json.dumps({'installed': [None]}))
elif scenario == 'markets':
    print(json.dumps({'marketplaces': [{'name': 'm'}]}))
else:
    print(json.dumps({'args': sys.argv[4:], 'home': os.environ['HOME'],
        'profile': os.environ['USERPROFILE'], 'codexHome': os.environ['CODEX_HOME'], 'cwd': os.getcwd()}))
'''


async def capture(name, operation, scenario):
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        home, codex = root / 'home', root / 'home/.codex'
        codex.mkdir(parents=True)
        script = root / 'fixture.py'
        script.write_text(PROGRAM)
        namespace['log_paths'] = SimpleNamespace(base=lambda: root / 'logs')
        obj = namespace['CodexNativo'](home, codex, timeout=0.15, close_timeout=0.02)
        obj._comando = lambda: [sys.executable, str(script), scenario]
        directories = []
        spawn = obj._spawn

        async def record_spawn(args, cwd, **pipes):
            directories.append(cwd)
            return await spawn(args, cwd, **pipes)

        obj._spawn = record_spawn
        if name == 'closed':
            await obj.close()

        async def invoke():
            if operation == 'add':
                return await obj.instalar_plugin('a@m')
            if operation == 'list':
                return await obj.plugins_instalados()
            if operation == 'markets':
                return await obj.marketplaces_instalados()
            if operation == 'upgrade':
                return await obj.atualizar_marketplace('m')
            return await obj.cli(['fixture', 'ação com espaço'])

        task = asyncio.create_task(invoke())
        if scenario in ('cancel', 'close'):
            while not obj._operations:
                await asyncio.sleep(0)
            await next(iter(obj._operations)).spawned.wait()
            if scenario == 'cancel':
                task.cancel()
            else:
                await obj.close()
        try:
            result = {'ok': await task}
        except asyncio.CancelledError:
            result = {'cancelled': True}
        except Exception as error:
            result = {'error': str(error), 'class': type(error).__name__}
        await obj.close()
        logs = [json.loads(path.read_text()) for path in sorted((root / 'logs').rglob('*.log'))]
        row = dict(name=name, operation=operation, scenario=scenario, result=result, logs=logs,
                   operations_empty=not obj._operations,
                   directories_removed=all(not Path(path).exists() for path in directories),
                   directories_outside_home=all(Path(path) != home for path in directories))
        encoded = json.dumps(row).replace(raw, '$ROOT')
        for directory in directories:
            encoded = encoded.replace(directory, '$ADMIN')
        return json.loads(encoded)


async def main():
    cases = [await capture(*args) for args in [
        ('command', 'cli', 'ok'), ('add', 'add', 'ok'), ('list', 'list', 'inventory'),
        ('markets', 'markets', 'markets'), ('bad-list', 'list', 'bad_inventory'),
        ('bad-markets', 'markets', 'ok'), ('upgrade', 'upgrade', 'ok'), ('auto', 'upgrade', 'auto'),
        ('exit', 'cli', 'exit'), ('invalid', 'cli', 'invalid'), ('array', 'cli', 'array'),
        ('errors', 'cli', 'errors'), ('timeout', 'cli', 'timeout'), ('cancel', 'cli', 'cancel'),
        ('close', 'cli', 'close'), ('closed', 'cli', 'ok')]]
    Path(__file__).with_name('native-cli.json').write_text(json.dumps({
        'source_commit': COMMIT, 'platform': os.name,
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_cli.py',
        'boundary': 'Executável controlado; CLI, árvore de processos, prazo, cancelamento, fechamento e limpeza originais. Identificador do diretório de diagnóstico fixado em fixture.',
        'program': PROGRAM, 'cases': cases}, ensure_ascii=True, indent=2) + '\n')
    print(len(cases))


asyncio.run(main())
