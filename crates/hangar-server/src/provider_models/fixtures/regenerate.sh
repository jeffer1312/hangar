#!/usr/bin/env bash
# Regenera somente o esperado, executando a revisão Python indicada pelas fixtures congeladas.
set -euo pipefail
reference="${1:?Informe a raiz do checkout Python da revisão de origem}"
fixtures="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
"${PYTHON:-python3}" - "$reference" "$fixtures" <<'PY'
import asyncio
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import AsyncMock, patch

reference, fixtures = map(Path, sys.argv[1:])
revision = subprocess.check_output(['git', '-C', str(reference), 'rev-parse', 'HEAD'], text=True).strip()
contract = json.loads((fixtures / 'contract.json').read_text())
if revision != contract['source_commit']:
    raise SystemExit('A referência Python precisa estar na revisão de origem registrada na fixture')
sys.path.insert(0, str(reference / 'backend'))

def load(name):
    path = reference / 'backend' / 'app' / (name + '.py')
    if not path.exists():
        raise SystemExit('Referência Python ausente: ' + name)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

pm, pc, km = map(load, ['pi_models', 'pi_catalog', 'kimi_models'])
for case in contract['cases']:
    op, value = case['op'], case['input']
    try:
        if op == 'parse_pi': answer = pc.parse(value)
        elif op == 'parse_omp': answer = pc.parse_omp(value)
        elif op == 'model_command': answer = pm.model_command(*value)
        elif op == 'think_command': answer = pm.think_command(value)
        elif op == 'confirms': answer = pm.confirms(*value)
        elif op == 'clean_alias': answer = km.clean_alias(value)
        elif op == 'kimi_catalog':
            with tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / 'config.toml'
                path.write_text(value, encoding='utf-8')
                answer = km.read_catalog(path)
        elif op in ('parse_switched', 'parse_thinking_set', 'parse_thinking_row'):
            answer = getattr(km, op)(value)
        else: raise SystemExit('Caso sem executor de referência: ' + op)
        case['expected'] = {'ok': answer}
    except (pm.PiModelError, km.KimiModelError) as error:
        case['expected'] = {'error': {'status': error.status, 'detail': error.detail}}

from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient
from app import api, terminal_input as ti

def save(name, data):
    data['generator'] = 'PYTHON=<Python com dependências do backend> bash crates/hangar-server/src/provider_models/fixtures/regenerate.sh <checkout da source_commit>'
    (fixtures / name).write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')

save('contract.json', contract)
driver = json.loads((fixtures / 'driver.json').read_text())
for case in driver['cases']:
    frames = iter(case['frames'])
    keys = []
    with patch.object(ti, 'deliverable', return_value=True), patch.object(ti, '_wait_input_ready', return_value=True), patch.object(ti.tmux, 'capture_pane', side_effect=lambda *a: next(frames)), patch.object(ti, 'send_keys', side_effect=lambda session, key, **kwargs: keys.append({'key': key, 'literal': kwargs.get('literal', False)})), patch.object(ti.time, 'sleep'):
        try: case['expected'] = {'ok': ti.TerminalInput().set_kimi_model('fixture', case['alias'], case['display'], case['effort'])}
        except ti.mp.PickerError as error: case['expected'] = {'error': {'status': error.status, 'detail': error.detail}}
    case['keys'] = keys
save('driver.json', driver)
readback = json.loads((fixtures / 'pi-readback.json').read_text())
for case in readback['cases']:
    with patch.object(api, '_pi_catalog', AsyncMock(return_value=(case['before'], '/x/a.jsonl'))), patch.object(api, '_session_config_dir', return_value=None), patch.object(api.terminal, 'send_pi_commands'), patch.object(api.pi_models, 'read_back', return_value=case['after']):
        try: case['expected'] = {'ok': asyncio.run(api.pi_model_set('fixture', api.PiModelBody(**case['body'])))}
        except HTTPException as error: case['expected'] = {'error': {'status': error.status_code, 'detail': error.detail}}
save('pi-readback.json', readback)
app = FastAPI()
@app.post('/pi')
def pi(body: api.PiModelBody): return body.model_dump()
@app.post('/kimi')
def kimi(body: api.KimiModelBody): return body.model_dump()
client = TestClient(app)
bodies = json.loads((fixtures / 'body.json').read_text())
for case in bodies['cases']:
    reply = client.post('/' + case['provider'], content=case['body'], headers={'content-type': 'application/json'})
    case['status'], case['expected'] = reply.status_code, reply.json()
save('body.json', bodies)
print('Contratos regenerados exclusivamente pela referência Python: ' + revision)
PY

"${PYTHON:-python3}" "$fixtures/characterize_extra.py" "$reference" "$fixtures/extra.json"
"${PYTHON:-python3}" "$fixtures/capture_legacy.py" "$reference" "$fixtures/legacy-calls.json"
