"""Captura opções da abertura e o fechamento conjunto de app-server e CLI."""
import ast
import asyncio
import json
from pathlib import Path
import sys
import tempfile

source = Path(__file__).with_name('generate_native_cli.py')
tree = ast.parse(source.read_text())
tree.body = [node for node in tree.body if not (
    isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)
    and isinstance(node.value.func, ast.Attribute) and node.value.func.attr == 'run')]
namespace = {'__file__': str(source)}
exec(compile(tree, str(source), 'exec'), namespace)
Native = namespace['namespace']['CodexNativo']
program = '''
import json, os, sys, time
from pathlib import Path
if 'app-server' in sys.argv:
    Path(sys.argv[1]).write_text(json.dumps({'argv': sys.argv[2:], 'cwd': os.getcwd()}))
    for line in sys.stdin:
        message = json.loads(line)
        if message.get('method') == 'initialize':
            print(json.dumps({'id': message['id'], 'result': {}}), flush=True)
else:
    time.sleep(30)
'''


async def capture(memory, command):
    with tempfile.TemporaryDirectory() as folder:
        root = Path(folder)
        home = root / 'home'
        codex = home / '.codex'
        codex.mkdir(parents=True)
        config = codex / 'config.toml'
        config.write_text('model = "fixture"\n')
        script = root / 'codex.py'
        script.write_text(program)
        observed = root / 'observed.json'
        client = Native(home, codex, memoria=memory, timeout=2, close_timeout=0.02)
        client._comando = lambda: [sys.executable, str(script), str(observed)]
        directories = []
        original_spawn = client._spawn
        async def spawn(args, cwd, **pipes):
            directories.append(cwd)
            return await original_spawn(args, cwd, **pipes)
        client._spawn = spawn
        await client.__aenter__()
        task = None
        if command:
            task = asyncio.create_task(client.cli(['fixture']))
            while not client._operations:
                await asyncio.sleep(0)
            await next(iter(client._operations)).spawned.wait()
        await client.close()
        result = None
        if task:
            try:
                result = {'ok': await task}
            except Exception as error:
                result = {'error': str(error)}
        observation = json.loads(observed.read_text())
        return dict(memory=memory, command=command, argv=observation['argv'],
                    cwd_outside_home=observation['cwd'] != str(home),
                    operations_empty=not client._operations,
                    directories_removed=all(not Path(path).exists() for path in directories),
                    config=config.read_text(), result=result)


async def main():
    # Diagnóstico bruto fica apenas na pasta descartável da captura.
    with tempfile.TemporaryDirectory() as logs:
        namespace['namespace']['log_paths'] = namespace['SimpleNamespace'](base=lambda: Path(logs))
        cases = [await capture(memory, command) for memory, command in
                 [(False, False), (True, False), (False, True)]]
    Path(__file__).with_name('native-owner.json').write_text(json.dumps({
        'source_commit': namespace['COMMIT'],
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_owner.py',
        'boundary': 'App-server e CLI controlados; abertura, argumentos e encerramento originais.',
        'program': program, 'cases': cases,
    }, ensure_ascii=True, indent=2) + '\n')
    print(len(cases))


asyncio.run(main())
