"""Captura as opções originais de sincronização sem importar o servidor."""
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
from types import ModuleType
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
namespace = dict(os=os)
for path, name in [('backend/app/config.py', 'automations_enabled'),
                   ('backend/app/codex_integracao.py', 'sincronizacao_ligada'),
                   ('backend/app/codex_instrucoes.py', '_sync_enabled')]:
    source = subprocess.check_output(['git', 'show', f'{COMMIT}:{path}'], cwd=ROOT, text=True)
    node = next(node for node in ast.parse(source).body if isinstance(node, ast.FunctionDef) and node.name == name)
    exec(compile(ast.Module(body=[node], type_ignores=[]), path, 'exec'), namespace)
app = ModuleType('app')
config = ModuleType('app.config')
integration = ModuleType('app.codex_integracao')
config.automations_enabled = namespace['automations_enabled']
integration.sincronizacao_ligada = namespace['sincronizacao_ligada']
cases = []
options = [True, False, None, 0, 1, '', 'false', [], [1], {}, {'x': 1}]
for env, sync, automations in [(env, True, True) for env in [None, '0', 'false', 'False', 'NO', 'no', 'off', '', ' false ', '1']]+[(None, option, True) for option in options]+[(None, True, option) for option in options]:
    runtime = ModuleType('app.runtime_config')
    runtime.get = {'codex_sync': sync, 'automations': automations}.get
    app.runtime_config = runtime
    with patch.dict(sys.modules, {'app': app, 'app.runtime_config': runtime, 'app.config': config, 'app.codex_integracao': integration}), patch.dict(os.environ, {}, clear=True):
        if env is not None:
            os.environ['CP_CODEX_SYNC_ENABLED'] = env
        cases.append(dict(env=env, sync=sync, automations=automations,
                          enabled=namespace['sincronizacao_ligada'](),
                          forced=namespace['_sync_enabled'](True)))
Path(__file__).with_name('sync-gate.json').write_text(json.dumps(dict(source_commit=COMMIT,
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_sync_gate.py',
    cases=cases), ensure_ascii=True, indent=2) + '\n')
print(len(cases))
