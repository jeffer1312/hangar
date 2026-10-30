"""Executa o reconciliador original com respostas nativas controladas e arquivos isolados."""
import ast
import asyncio
import contextlib
import copy
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
from types import SimpleNamespace
from urllib.parse import urlsplit

loader = Path(__file__).with_name('generate_instructions.py')
scope = {'__file__': str(loader)}
exec(compile(loader.read_text().split('cases=[]', 1)[0], str(loader), 'exec'), scope)
namespace = scope['ns']
ROOT, COMMIT = scope['ROOT'], scope['COMMIT']
namespace.update(asyncio=asyncio, contextlib=contextlib, copy=copy, datetime=datetime,
                 timezone=timezone, re=re, urlsplit=urlsplit, time=SimpleNamespace(time=lambda: 100000.0),
                 _INTERVALO=21600, _log=SimpleNamespace(warning=lambda *args: None))
messages = subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_msgs.py'], cwd=ROOT, text=True)
exec(compile(messages, 'codex_msgs_original.py', 'exec'), namespace)
importer = ast.parse(subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_importador.py'], cwd=ROOT, text=True))
error = next(node for node in importer.body if isinstance(node, ast.ClassDef) and node.name == 'CodexNativoErro')
exec(compile(ast.Module(body=[error], type_ignores=[]), 'codex_importador_original.py', 'exec'), namespace)
source = ast.parse(subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_integracao.py'], cwd=ROOT, text=True))
functions = {'_iso', '_toml', '_chave', '_plugins_desejados', '_origem_marketplace', '_identidade_plugin'}
methods = {'_plugins', '_reconciliar_plugins', '_habilitar_plugins', '_mutacao', '_checkpoint', '_etapa', '_sub', '_erro'}
nodes = []
for node in source.body:
    if isinstance(node, ast.FunctionDef) and node.name in functions:
        nodes.append(node)
    elif isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == '_ID' for target in node.targets):
        nodes.append(node)
    elif isinstance(node, ast.ClassDef) and node.name == 'IntegracaoCodex':
        node.body = [method for method in node.body if isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef)) and method.name in methods]
        nodes.append(node)
exec(compile(ast.Module(body=nodes, type_ignores=[]), 'codex_integracao_original.py', 'exec'), namespace)


def snapshot(root):
    result = {}
    for path in sorted(root.rglob('*')):
        if not path.is_file():
            continue
        data = path.read_bytes()
        key = str(path.relative_to(root))
        if key.startswith('.codex/backups/'):
            backup = json.loads(data)
            original = Path(backup['path'])
            expected_name = namespace['hash_bytes'](str(original.absolute()).encode()) + '.json'
            assert path.name == expected_name, (path.name, expected_name)
            key = '.codex/backups/$HASH/' + str(original.relative_to(root))
        assert key not in result
        result[key] = data.replace(str(root).encode(), b'$ROOT').hex()
    return result


async def capture(name):
    with tempfile.TemporaryDirectory() as raw:
        home = Path(raw)
        codex = home / '.codex'
        plugin = codex / 'plugin'
        for path in [home / '.claude/plugins', codex, plugin, codex / '.hangar-integracao']:
            path.mkdir(parents=True, exist_ok=True)
        (plugin / 'personal.txt').write_text('Pessoal\n')
        settings = home / '.claude/settings.json'
        selected = {} if name in ('empty', 'removed') else {'a@m': True}
        settings.write_text(json.dumps({'enabledPlugins': None if name == 'invalid_enabled' else selected}))
        if name == 'source_missing':
            settings.unlink()
        known = {} if name == 'unknown_market' else {'m': {'source': {'source': 'github', 'repo': 'Org/Repo'}}}
        (home / '.claude/plugins/known_marketplaces.json').write_text(json.dumps(known))
        source_url = 'https://github.com/Other/Repo' if name == 'homonym' else 'https://github.com/Org/Repo'
        config = codex / 'config.toml'
        config.write_text('[marketplaces.m]\nsource_type = "git"\nsource = ' + json.dumps(source_url) + '\n[plugins."a@m"]\nenabled = true\n')
        identity = {'path': str(plugin), 'versao': '1', 'origem': 'm', 'id_codex': 'a@m'}
        registry = {'plugins': {} if name in ('empty', 'missing', 'import_partial') else {'a@m': identity},
                    'marketplaces_em': 100000.0, 'marketplaces_tentativa_em': 100000.0}
        if name in ('pending_market_retry', 'pending_market_recent'):
            registry['marketplaces_pendentes'] = ['m']
            registry['marketplaces_em'] = registry['marketplaces_tentativa_em'] = 99000.0 if name.endswith('retry') else 99999.0
        if name == 'pending_plugin':
            registry['plugins_pendentes'] = ['a@m', 'removed@m']
        initial_registry = copy.deepcopy(registry)
        before = snapshot(home)
        calls, checkpoints, config_edits = [], [], []
        inventory = [] if name in ('missing', 'import_partial') else [{'pluginId': 'a@m', 'version': '1'}]
        changed = False
        obj = namespace['IntegracaoCodex']()
        obj.home, obj.codex_home = home, codex
        obj.raiz, obj.backups = codex / '.hangar-integracao', codex / 'backups'
        obj._estado = {'erros': [], 'avisos': [], 'progresso': {'passo': 3, 'total': 9, 'sub': None}}
        obj._plugins_confirmados = set()

        def status():
            result = copy.deepcopy(obj._estado)
            for field in ('erros', 'avisos'):
                result[field] = [namespace['serializar'](item) for item in result[field]]
            if 'etapa' in result:
                result['etapa'] = namespace['serializar'](result['etapa'])
            return result

        obj.status = status
        original_checkpoint = obj._checkpoint

        def checkpoint(registry):
            original_checkpoint(registry)
            checkpoints.append(copy.deepcopy(registry))

        obj._checkpoint = checkpoint

        def hooks(path):
            nonlocal changed
            calls.append(['hooks', str(path)])
            if name == 'hook_failure':
                raise OSError('falha de hook')
            if name == 'source_changes_forever' or (name == 'source_changed' and not changed):
                settings.write_bytes(settings.read_bytes() + b' ')
                changed = True

        obj._hooks_plugin = hooks

        async def edit(prepare):
            edits, confirm = prepare(tomllib.loads(config.read_text()))
            config_edits.append(edits)
            confirm()

        obj._editar_config = edit

        class Native:
            async def plugins_instalados(self):
                calls.append(['list'])
                return copy.deepcopy(inventory)

            async def detectar(self):
                calls.append(['detect'])
                return [{'itemType': 'CONFIG'}, {'itemType': 'PLUGINS', 'description': 'Plugins', 'details': {
                    'plugins': [{'marketplaceName': 'm', 'pluginNames': ['a', 'unrelated'], 'extra': 'preserved'}]}}]

            async def importar(self, items):
                calls.append(['import', items])
                inventory[:] = [{'pluginId': 'a@m', 'version': '1'}]
                return {'itemTypeResults': [{'failures': [{'message': 'Falha parcial'}] if name == 'import_partial' else []}]}

            async def atualizar_marketplace(self, market):
                calls.append(['upgrade', market])
                if name == 'upgrade_failure':
                    raise RuntimeError('falha de atualização')
                return {'errors': []}

            async def instalar_plugin(self, identity):
                calls.append(['add', identity])
                if name == 'install_failure':
                    raise RuntimeError('falha de instalação')
                return {'installedPath': str(plugin), 'version': '1'}

        forced = name in ('force', 'upgrade_failure', 'install_failure')
        try:
            await obj._plugins(Native(), {'ignorado@caller'}, registry, forced)
            result = {'ok': True}
        except Exception as error:
            result = {'error': str(error), 'class': type(error).__name__}
        row = dict(name=name, forced=forced, before=before, registry_before=initial_registry,
                   result=result, registry=registry, state=status(), confirmed=sorted(obj._plugins_confirmados),
                   calls=calls, checkpoints=checkpoints, config_edits=config_edits, after=snapshot(home))
        return json.loads(json.dumps(row).replace(raw, '$ROOT'))


async def main():
    names = ['empty', 'stable', 'force', 'missing', 'unknown_market', 'homonym', 'upgrade_failure',
             'install_failure', 'hook_failure', 'pending_market_retry', 'pending_market_recent',
             'pending_plugin', 'removed', 'source_changed', 'source_changes_forever', 'source_missing',
             'invalid_enabled', 'import_partial']
    cases = [await capture(name) for name in names]
    Path(__file__).with_name('plugin-reconciliation.json').write_text(json.dumps({
        'source_commit': COMMIT,
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_plugin_reconciliation.py',
        'boundary': 'Respostas nativas, adaptador de hooks e confirmação dos edits são controlados; seleção, retentativa, checkpoint e arquivos são originais. A raiz temporária é normalizada antes do hex. O nome do backup é conferido contra SHA256 da origem absoluta e representado pela origem sob $HASH.',
        'cases': cases}, ensure_ascii=True, indent=2) + '\n')
    print(len(cases))


asyncio.run(main())
