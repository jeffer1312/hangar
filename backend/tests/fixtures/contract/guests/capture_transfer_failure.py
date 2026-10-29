"""Executa a fronteira original da transferência sem runtime nem sessão viva."""
import ast
import asyncio
import json
import logging
import subprocess
import sys
from pathlib import Path
from types import ModuleType,SimpleNamespace
HERE=Path(__file__).resolve().parent
source=HERE.parents[3]/'app/api.py'
node=next(n for n in ast.parse(source.read_text()).body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_durante_troca')
code=compile(ast.Module(body=[node],type_ignores=[]),str(source),'exec')
app=ModuleType('app');app.runtime_coordinator=SimpleNamespace(current=lambda:None);sys.modules['app']=app
rows=[]
for fail in [False,True]:
    calls=[]
    def set_life(name,life):
        calls.append(['guest_life',name,life])
        if fail:raise OSError('falha isolada')
    bridge=SimpleNamespace(changing_mode=set(),mode_change=lambda name,on:calls.append(['mode',name,on]),set_life=lambda name,life:calls.append(['share_life',name,life]))
    logger=logging.getLogger('fixture');logger.addHandler(logging.NullHandler());logger.propagate=False
    ns={'asyncio':asyncio,'share_bridge':bridge,'session_life':lambda _: 'k:new','guest_users':SimpleNamespace(set_life=set_life),
        '_log':logger,'_invalidate_lists':lambda:calls.append(['invalidate'])}
    exec(code,ns)
    async def change():return {'ok':True}
    result=asyncio.run(ns['_durante_troca']('session',change()))
    rows.append({'guest_write_fails':fail,'result':result,'calls':calls,'changing':sorted(bridge.changing_mode)})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_transfer_failure.py','rows':rows}
(HERE/'transfer-failure.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print('2 fronteiras de transferência capturadas')
