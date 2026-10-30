"""Caracteriza callbacks artificiais e a recusa dos três provedores no escritor oficial."""
import ast
import json
import subprocess
import threading
from pathlib import Path
from types import SimpleNamespace as NS
from fastapi import HTTPException

source = Path('app/api.py')
tree = ast.parse(source.read_text())
nodes = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in ('_on_hook_transition', 'loop_create')]
for node in nodes:
    node.decorator_list = []
    node.returns = None
    for arg in node.args.args:
        arg.annotation = None
rows = []
for provider in ('pi', 'omp', 'kimi'):
    for state in ('working', 'awaiting_input', 'idle'):
        for active in (False, True):
            for sent in (0, 1):
                calls = []
                info = NS(name='fixture', jsonl='fixture.jsonl', provider=provider)
                namespace = {
                    'registry': NS(list=lambda: [info]), 'session_key': lambda path: 'sid',
                    '_rust_conduz': lambda row: False,
                    'threading': NS(Thread=lambda target, **kw: NS(start=target)),
                    'hook_state': NS(get_state=lambda sid: ('idle', 1)),
                    'corrige_ocioso_kimi': lambda marker, path: marker,
                    'loop_mod': NS(_lock=threading.Lock(), ACTIVE={'running', 'paused_awaiting', 'done_claimed'},
                        LoopLink=lambda name: NS(get=lambda: {'status': 'running'} if active else None,
                            update=lambda **fields: calls.append(['pause', fields])),
                        schedule_tick=lambda name, factory: calls.append(['tick', name])),
                    '_drenar': lambda *args: calls.append(['drain']) or sent,
                    '_agendar_confirmacao': lambda *args: calls.append(['confirm']),
                    '_maybe_chain': lambda name: calls.append(['then', name]),
                    '_CONFIRM_GRACE': 1, '_falhas_seguidas': {},
                    'HTTPException': HTTPException, 'erro': lambda code, msg: {'code':code, 'params':{}, 'msg':msg},
                }
                exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), 'exec'), namespace)
                namespace['_on_hook_transition']('sid', state)
                rows.append({'provider':provider, 'state':state, 'active':active, 'sent':sent, 'calls':calls})
    try:
        namespace['loop_create']('fixture', NS())
    except HTTPException as exc:
        rows.append({'provider':provider, 'create_status':exc.status_code, 'create_detail':exc.detail})
result = {'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_hook_consumers.py',
    'isolation':'Funções originais por AST; registro, relógio, marcador, fila e agendamento substituídos por fontes em memória. Loop ativo em Pi/omp/Kimi é cenário artificial; escritor oficial é executado e recusa esses provedores. Não é prova de evento real ou de transcript Kimi.', 'rows':rows}
Path(__file__).with_name('loop-hook-consumers.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
print(len(rows), 'casos capturados')
