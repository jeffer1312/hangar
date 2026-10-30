"""Captura edição nativa de configuração e confirmação do manifesto após a escrita."""
import ast,asyncio,io,json,subprocess,sys,tarfile,tempfile
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True)
tree=ast.parse(source)
cls=next(n for n in tree.body if isinstance(n,ast.ClassDef) and n.name=='IntegracaoCodex')
method=next(n for n in cls.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_config')
key=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='_chave')
ns=dict(limite_instrucoes=lambda _:40000,json=json)
exec(compile(ast.Module(body=[key,method],type_ignores=[]),'backend/app/codex_integracao.py','exec'),ns)
cases=[]
inputs=[('base',{}),('fallbacks-existing',{'project_doc_fallback_filenames':['README.md','CLAUDE.md']})]
for field in ['project_doc_fallback_filenames','project_doc_max_bytes','features','mcp_servers','agents','shell_environment_policy']:
 for value in [None,False,0,100000,'text',[],{},['CLAUDE.md']]:inputs.append((field,{field:value}))
inputs.extend([('mcp-stale',{'mcp_servers':{'old':{'enabled':False}}}),('mcp-personal',{'mcp_servers':{'personal':{'enabled':False}}}),('env-personal',{'shell_environment_policy':{'set':{'USER':'personal'}}})])
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw);data=subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT)
 with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
 sys.path.insert(0,str(archive/'backend'))
 from app.codex_msgs import serializar
 for name,current in inputs:
  for confirm in [False,True]:
   registry={'mcp_servers':{'old':{'command':'old'}}};state={'avisos':[]};edits=[]
   async def edit(callback):
    values,done=callback(current);edits.extend(values)
    if confirm:done()
   obj=SimpleNamespace(codex_home=Path('codex'),_estado=state,_editar_config=edit)
   try:asyncio.run(ns['_config'](obj,None,{}, {},hooks=True,memoria=True,registro=registry,env={'NEW':'source'}));result={'ok':None}
   except Exception as error:result={'error':type(error).__name__,'message':str(error)}
   cases.append(dict(name=name,current=current,confirm=confirm,result=result,edits=edits,registry=registry,warnings=[serializar(v) for v in state['avisos']]))
Path(__file__).with_name('config-edits.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_config_edits.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
