"""Caracteriza contexto de loop retido durante troca; nenhum comando ou sessão real."""
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
sys.path.insert(0, str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    from app import loop as loop_mod
    source = Path('app/api.py')
    node = next(node for node in ast.parse(source.read_text()).body if isinstance(node, ast.FunctionDef) and node.name == '_loop_ctx')
    queued, drained, checks, pushed = [], [], [], []
    class Queue:
        def __init__(self, name): pass
        def append(self, text, delivered=False): queued.append({'text': text, 'delivered': delivered})
        def entry_delivered(self, entry): return True
    info = SimpleNamespace(name='s', provider='claude', cwd='pasta-antiga', jsonl='vida-antiga.jsonl')
    ns = dict(loop_mod=loop_mod, registry=SimpleNamespace(list=lambda: [info]), PromptQueue=Queue,
              _drenar=lambda *args: drained.append(list(args)), automations_enabled=lambda: True,
              branch_of=lambda _: 'feature', last_assistant_text=lambda _: '',
              push=SimpleNamespace(notify_loop=lambda *args: pushed.append(list(args))))
    exec(compile(ast.Module(body=[node], type_ignores=[]), str(source), 'exec'), ns)
    rows = []
    with patch.object(loop_mod, 'time', SimpleNamespace(time=lambda: 1000.0)), patch.object(loop_mod, '_run_check', lambda cmd, cwd: (checks.append([cmd, cwd]) or (1, 'conferência simulada'))):
        for provider in ('claude', 'codex', 'pi', 'omp', 'kimi'):
            info.provider, info.cwd, info.jsonl = 'claude', 'pasta-antiga', 'vida-antiga.jsonl'
            initial = loop_mod.new_loop('objetivo', 'comando-simulado', 10, False)
            initial['goal_delivered_ts'] = 1.0
            link = loop_mod.LoopLink('s'); link.set(initial)
            before = link.path.read_bytes()
            ctx = ns['_loop_ctx']('s')
            info.provider, info.cwd, info.jsonl = provider, 'pasta-nova', 'vida-nova.jsonl'
            queued.clear(); drained.clear(); checks.clear(); pushed.clear()
            result = loop_mod.run_tick('s', ctx)
            rows.append(dict(current_provider=provider, selected_jsonl='vida-antiga.jsonl', current_jsonl=info.jsonl,
                             checks=list(checks), queued=list(queued), drained=list(drained), pushed=list(pushed),
                             unchanged=before == link.path.read_bytes(), result=result, stored=link.get()))
    result = dict(source_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                  command='cd backend && uv run python tests/fixtures/contract/guests/capture_loop_life_race.py',
                  isolation='Intercalação determinística entre _loop_ctx original e run_tick original; registro mutável, comandos/fila/drain/push simulados, LoopLink real em temporário; sem servidor, subprocesso de verificação ou sessão real.', rows=rows)
    Path(__file__).with_name('loop-life-race.json').write_text(json.dumps(result, ensure_ascii=True, indent=2) + '\n')
    print(f'{len(rows)} intercalações capturadas')
