"""Captura mensagens estruturadas da integração e extrai seu catálogo para a migração."""
import json,string,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
ns={}
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_msgs.py'],cwd=ROOT,text=True)
exec(compile(source,'backend/app/codex_msgs.py','exec'),ns)
cases=[]
for code,template in ns['CATALOGO'].items():
 names=[field for _,field,_,_ in string.Formatter().parse(template) if field]
 for value in ['fixture',None,True,3,1.25,['x'],{'x':1},'{tipo} {etapa}']:
  params={name:value for name in names}
  expected=ns['serializar'](ns['msg'](code,**params))
  cases.append(dict(code=code,params=params,expected=expected))
Path(__file__).with_name('messages.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_messages.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
(ROOT/'crates/hangar-server/src/codex_setup/messages.json').write_text(json.dumps(ns['CATALOGO'],ensure_ascii=False,indent=2)+'\n')
print(len(cases))
