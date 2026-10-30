"""Captura a mescla original de fragmentos, incluindo campos particulares e proveniência."""
import ast, copy, itertools, json, subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
ns={'deepcopy':copy.deepcopy,'_AUSENTE':object()}
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_msgs.py'],cwd=ROOT,text=True)
exec(compile(source,'backend/app/codex_msgs.py','exec'),ns)
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_fragmentos.py'],cwd=ROOT,text=True)
nodes=[n for n in ast.parse(source).body if isinstance(n,ast.FunctionDef) and n.name in {'_tirar_gerenciado','_mesclar_campos','mesclar_config'}]
exec(compile(ast.Module(body=nodes,type_ignores=[]),'backend/app/codex_fragmentos.py','exec'),ns)
values=[{}, {'fixture':None},{'fixture':False},{'fixture':0},{'fixture':1},{'fixture':True},{'fixture':'pessoal'},
 {'fixture':[]},{'fixture':[1,True]}, {'fixture':{'a':1}},{'fixture':{'a':1,'local':2}},
 {'fixture':{'a':2,'nested':{'x':1,'local':2}}},{'fixture':{'nested':{'x':2}},'other':{'enabled':True}},
 None,[]]
cases=[]
for current,source,previous in itertools.product(values,repeat=3):
 for trusted in [[],['fixture']]:
  try:
   merged,manifest,warnings=ns['mesclar_config'](current,source,previous,confiaveis=set(trusted))
   result={'ok':{'merged':merged,'manifest':manifest,'warnings':[ns['serializar'](item) for item in warnings]}}
  except Exception as error:result={'error':type(error).__name__,'message':str(error)}
  cases.append(dict(current=current,source=source,previous=previous,trusted=trusted,result=result))
Path(__file__).with_name('fragments.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_fragments.py',cases=cases),ensure_ascii=True,separators=(',',':'))+'\n')
print(len(cases))
