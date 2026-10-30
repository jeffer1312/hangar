"""Caracteriza a retomada de vínculo herdado, com persistência real e transporte simulado."""
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict

sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    from app import loop as loop_mod
    from app.mensagens import erro
    source = Path('app/api.py')
    nodes = []
    for node in ast.parse(source.read_text()).body:
        if isinstance(node, ast.ClassDef) and node.name in ('_StrictBody', 'LoopResolve'):
            nodes.append(node)
        elif isinstance(node, ast.FunctionDef) and node.name in ('_loop_ctx', 'loop_resolve'):
            node.decorator_list = []
            nodes.append(node)
    queued, drained, pushed = [], [], []
    info = SimpleNamespace(name='s', provider='claude', jsonl='fixture.jsonl', cwd=None)
    class Queue:
        def __init__(self, name): pass
        def append(self, text, delivered=False): queued.append({'text': text, 'delivered': delivered})
        def entry_delivered(self, entry): return True
    ns = dict(BaseModel=BaseModel, ConfigDict=ConfigDict, HTTPException=HTTPException, loop_mod=loop_mod,
              erro=erro, registry=SimpleNamespace(list=lambda: [info]), PromptQueue=Queue,
              _drenar=lambda *args: drained.append(list(args)), automations_enabled=lambda: True,
              branch_of=lambda _: None, last_assistant_text=lambda _: None,
              push=SimpleNamespace(notify_loop=lambda *args: pushed.append(list(args))))
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), 'exec'), ns)
    app = FastAPI()
    app.add_api_route('/api/sessions/{name}/loop/resolve', ns['loop_resolve'], methods=['POST'])
    rows = []
    with TestClient(app, raise_server_exceptions=False) as client, patch.object(loop_mod, 'time', SimpleNamespace(time=lambda: 1000.0)):
        for provider in ('claude', 'pi', 'omp', 'kimi', 'codex'):
            for accept in (False, True):
                for maximum in (0, 10):
                    info.provider = provider
                    initial = loop_mod.new_loop('objetivo', None, maximum, False)
                    initial['status'] = 'done_claimed'
                    link = loop_mod.LoopLink('s')
                    link.set(initial)
                    before = link.path.read_bytes()
                    queued.clear(); drained.clear(); pushed.clear()
                    response = client.post('/api/sessions/s/loop/resolve', json={'accept': accept})
                    rows.append(dict(provider=provider, accept=accept, initial=initial, status=response.status_code,
                                     response=response.json(), unchanged=before == link.path.read_bytes(),
                                     queued=list(queued), drained=list(drained), pushed=list(pushed)))
    result = dict(source_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                  command='cd backend && uv run python tests/fixtures/contract/guests/capture_loop_resolve_provider.py',
                  isolation='Handler e _loop_ctx originais por AST; LoopLink real em temporário; registro, fila, drain e push simulados; sem autenticação ou sessão real.', rows=rows)
    Path(__file__).with_name('loop-resolve-provider.json').write_text(json.dumps(result, ensure_ascii=True, indent=2) + '\n')
    print(f'{len(rows)} casos capturados')
