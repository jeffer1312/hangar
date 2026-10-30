"""Captura seleção e identidade dos plugins no reconciliador original."""
import ast
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
source = subprocess.check_output(['git', 'show', f'{COMMIT}:backend/app/codex_integracao.py'], cwd=ROOT, text=True)
nodes = []
for node in ast.parse(source).body:
    if isinstance(node, ast.FunctionDef) and node.name in {'_plugins_desejados', '_origem_marketplace', '_identidade_plugin'}:
        nodes.append(node)
    if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == '_ID' for target in node.targets):
        nodes.append(node)
namespace = dict(Path=Path, os=os, re=re, urlsplit=urlsplit)
exec(compile(ast.Module(body=nodes, type_ignores=[]), 'codex_integracao_original.py', 'exec'), namespace)
cases = []
USERINFO_A = 'user' + ':' + 'secret'
USERINFO_B = 'user' + ':' + 'password'


def capture(operation, args, **kwargs):
    try:
        result = namespace[operation](*args, **kwargs)
        if isinstance(result, set):
            result = sorted(result)
        result = {'ok': result}
    except Exception as error:
        result = {'error': str(error), 'class': type(error).__name__}
    cases.append(dict(operation=operation, args=args, kwargs=kwargs, result=result))


for settings in [{}, {'enabledPlugins': None}, {'enabledPlugins': []}, {'enabledPlugins': {}},
                 {'enabledPlugins': {'a@m': True, 'b@m': False}}, {'enabledPlugins': {'a@m': 1}},
                 {'enabledPlugins': {'a@m': 'true'}}, {'enabledPlugins': {'sem-mercado': True}},
                 {'enabledPlugins': {'ação@m': True}}, {'enabledPlugins': {'a@m\n': True}},
                 {'enabledPlugins': {'a-b.c_d@m-n.o_p': True}}, {'enabledPlugins': {'a@@m': True}}]:
    capture('_plugins_desejados', [settings])

with tempfile.TemporaryDirectory() as raw:
    previous = os.getcwd()
    os.chdir(raw)
    try:
        for source in [{}, {'source': 'github', 'repo': 'Org/Repo'}, {'source': 'github', 'repo': None},
                       {'source': 'github'}, {'source': 'directory', 'path': './plugins/../market'},
                       {'source': 'git', 'url': 'git@EXAMPLE.org:Org/Repo.git'},
                       {'source': 'git', 'url': f'https://{USERINFO_A}@example.org/Org/Repo.git/'},
                       {'source': 'git', 'url': 'ssh://git@EXAMPLE.org/Org/Repo.git'},
                       {'source': 'git', 'url': '/relative'}, {'source': 'git', 'url': 'http://['},
                       {'source': 'git', 'url': 3}, {'source': 'other', 'url': 'https://example.org/a'},
                       None, [], 'invalid']:
            capture('_origem_marketplace', [{'source': source}], claude=True)
        for source in [{}, {'source_type': 'local', 'source': './market'},
                       {'source_type': 'git', 'source': 'https://example.org/Org/Repo.git'},
                       {'source_type': 'github', 'repo': 'Org/Repo'},
                       {'source_type': 'directory', 'source': 'missing'}, {'source_type': 'git', 'source': ''}]:
            capture('_origem_marketplace', [source])
        for url in [f'https://{USERINFO_B}@EXAMPLE.org:invalid/./../Repo%20x.git?query#fragment',
                    'https://example.org/A//B.git/', '//EXAMPLE.org/Repo.git',
                    ' https://EXAMPLE.org/Repo.git', 'https://EXAM\tPLE.org/Repo.git',
                    'https://example.org/Repo.GIT', 'https://example.org/Repo.git ',
                    'http://[::1]/repo', 'http://[fe80::1%ZoNe]/repo',
                    'http://[fe80::1%]/repo', 'http://[fe80::1%a%b]/repo',
                    'http://[127.0.0.1]/repo', 'http://[invalid]/repo',
                    'http://[v1.future]/repo', 'http://[v.future]/repo',
                    'http://prefix[::1]/repo', 'http://[::1]suffix/repo',
                    'http://[user]@example.org/repo', 'http://exa\u2100mple.org/repo',
                    'git@EXAMPLE.org:Org/Repo.git', 'git@[invalid]:Repo.git']:
            capture('_origem_marketplace', [{'source_type': 'git', 'source': url}])
        origin = ('git', 'example.org/Org/Repo')
        same = {'source_type': 'git', 'source': 'https://example.org/Org/Repo.git'}
        other = {'source_type': 'git', 'source': 'https://example.org/Other/Repo.git'}
        for markets, inventory in [({}, {}), ({'m': same}, {'a@m': {}}), ({'m': other}, {'a@m': {}}),
                                   ({'alias': same}, {'a@alias': {}}),
                                   ({'alias': same, 'second': same}, {'a@alias': {}, 'a@second': {}}),
                                   ({'alias': other}, {'a@alias': {}}),
                                   ({'m': same, 'alias': same}, {'a@m': {}, 'a@alias': {}}),
                                   ({'alias': same}, {'b@alias': {}})]:
            capture('_identidade_plugin', ['a@m', origin, markets, inventory])
        # As credenciais fictícias são recompostas pelo teste, sem URLs de senha no corpus.
        serialized = (json.dumps(cases).replace(raw, '$ROOT')
                      .replace(USERINFO_A + '@', '$USERINFO_A@')
                      .replace(USERINFO_B + '@', '$USERINFO_B@'))
    finally:
        os.chdir(previous)
Path(__file__).with_name('plugin-selection.json').write_text(json.dumps({
    'source_commit': COMMIT,
    'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_plugin_selection.py',
    'cases': json.loads(serialized)}, ensure_ascii=True, indent=2) + '\n')
print(len(cases))
