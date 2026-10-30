"""Caracteriza silêncio e ordem do porteiro de avisos original, sem enviar notificações."""
import ast
from datetime import datetime, time as dtime
import itertools
import json
from pathlib import Path
import subprocess

source=Path('app/push.py')
nodes=[node for node in ast.parse(source.read_text()).body if isinstance(node,ast.FunctionDef) and node.name in {'_in_quiet_hours','_suppressed'}]
ns={'dtime':dtime,'datetime':datetime}
exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),ns)
rows=[]
windows=[None,{},False,0,1,[],[1],'texto',{'start':'22:00','end':'07:00'}, {'start':'12:00','end':'12:00'}]
for value in ['00','0000','T12:30','t12:30','24:00','12.5','12:30.5','12:30:59.','12:30Z','12:30+01:00','12:30-00:00:30','23:59:59,5','25:00',None,1]:
    windows.append({'start':value,'end':'07:00'})
for window, now in itertools.product(windows,['00:00','06:59:59.999999','07:00','12:30','23:59:59.999999']):
    ns['_load_prefs']=lambda:{'quiet_hours':window}
    try: result={'value':ns['_in_quiet_hours'](dtime.fromisoformat(now))}
    except Exception as error: result={'error':type(error).__name__}
    rows.append({'window':window,'now':now,**result})
order=[]
for flags in itertools.product([False,True],repeat=4):
    reads=[]
    for name,value in zip(['is_muted','_in_quiet_hours','_hidden_guest','_owner_at_pc'],flags):
        def read(*args,name=name,value=value):reads.append(name);return value
        ns[name]=read
    order.append({'flags':flags,'value':ns['_suppressed']('s'),'reads':reads})
out=dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),command='cd backend && uv run python tests/fixtures/contract/guests/capture_push_gate.py',isolation='Funções originais por AST; datetime.time real, preferências e leitores de decisão controlados. Sem arquivo de convidados, presença ou rede.',windows=rows,order=order)
Path(__file__).with_name('push-gate.json').write_text(json.dumps(out,ensure_ascii=True,indent=2)+'\n')
print(len(rows),len(order))
