"""Captura a confiança inicial de hooks na preparação única introduzida na main."""
import ast
import json
from pathlib import Path
import subprocess
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_compat.py'],cwd=ROOT,text=True)
node=next(n for n in ast.parse(source).body if isinstance(n,ast.FunctionDef) and n.name=='edicoes_confianca_hooks')
ns=dict(Path=Path,json=json)
exec(compile(ast.Module(body=[node],type_ignores=[]),'original-hook-trust','exec'),ns)
cases=[]
def capture(name,data,reload=False):
    with patch.object(Path,'home',return_value=Path('/fixture/home')):
        try:
            edits,manual=ns[node.name](data,Path('/fixture/account'),recarregar=reload)
            result=dict(edits=edits,manual=manual)
        except Exception as error:result=dict(error=type(error).__name__,message=str(error))
    cases.append(dict(name=name,data=data,reload=reload,result=result))
for value in [None,[],{},dict(data=None),dict(data={}),dict(data=[]),dict(data=[None]),dict(data=[{}]),
              dict(data=[dict(errors=['failure'])]),dict(data=[dict(warnings=['warning'])]),
              dict(data=[dict(hooks={})]),dict(data=[dict(hooks=[None])])]:capture('shape',value)
for status in ['untrusted','modified','trusted','other',None]:
    for enabled in [True,False,1,'true',None]:
        for source_name,key in [('user','project:hook'),('plugin','project:hook'),('project','project:hook'),
                                ('project','/fixture/account/hooks.json:id'),('project','/fixture/home/.codex/hooks.json:id'),
                                ('project','\\fixture\\account\\hooks.json:id')]:
            for reload in [False,True]:
                hook=dict(trustStatus=status,enabled=enabled,source=source_name,key=key,currentHash='hash')
                capture('hook',dict(data=[dict(hooks=[hook])]),reload)
for key,current in [(None,'hash'),('key',None),(1,'hash'),('key',1),('quote"\n','hash'),('',''),('á😀','hash'),('/fixture/account/hooks.jsonx:id','hash')]:
    capture('identity',dict(data=[dict(hooks=[dict(trustStatus='untrusted',enabled=True,source='plugin',key=key,currentHash=current)])]))
Path(__file__).with_name('hook-trust.json').write_text(json.dumps(dict(source_commit=COMMIT,
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_hook_trust.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(f'{len(cases)} casos originais de confiança capturados')
