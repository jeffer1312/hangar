"""Captura os edits nativos da seleção de plugins, inclusive a chamada sem escolhas."""
import ast,asyncio,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True);tree=ast.parse(source)
key=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='_chave')
cls=next(n for n in tree.body if isinstance(n,ast.ClassDef) and n.name=='IntegracaoCodex');method=next(n for n in cls.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_habilitar_plugins')
ns={'json':json};exec(compile(ast.Module(body=[key,method],type_ignores=[]),'original-plugin-edits','exec'),ns)
configs=[{}, {'plugins':None},{'plugins':[]},{'plugins':True},{'plugins':'text'}]
for entry in [None,[],False,3,'text',{},*({'enabled':v} for v in [None,False,True,0,1,1.0,'true',[],{}])]:configs.append({'plugins':{'one@market':entry}})
cases=[]
async def run():
 for current in configs:
  for choices in [{},{'one@market':True,'two@market':False},{'a."/ação':True}]:
   calls=[]
   async def edit(prepare):
    calls.append('edit_config');edits,confirm=prepare(current);calls.append(edits);confirm()
   obj=SimpleNamespace(_editar_config=edit)
   try:await ns['_habilitar_plugins'](obj,None,choices);result={'ok':True,'calls':calls}
   except Exception as error:result={'error':type(error).__name__,'message':str(error),'calls':calls}
   cases.append(dict(current=current,choices=choices,result=result))
asyncio.run(run())
file_cases=[]
with tempfile.TemporaryDirectory() as raw:
 root=Path(raw);archive=root/'archive';archive.mkdir();data=subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT)
 with tarfile.open(fileobj=io.BytesIO(data)) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(root/'home');(root/'home').mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_arquivos as original_files
 for number in [10**90,-10**90,2**127]:
  with tempfile.TemporaryDirectory() as folder:
   base=Path(folder);path=base/'config.toml';text='extra = '+str(number)+'\n';path.write_text(text);events=[]
   def native(*args,**kwargs):events.append({'native':True});raise AssertionError('Native não deve abrir sem edits')
   def prepare(current):events.append({'prepare':current});return [],lambda:events.append({'confirm':True})
   try:asyncio.run(original_files.editar_config(path,base/'backup',base/'work',native,prepare));result={'ok':True,'events':events,'unchanged':path.read_text()==text}
   except Exception as error:result={'error':type(error).__name__,'message':str(error),'events':events,'unchanged':path.read_text()==text}
   file_cases.append(dict(toml=text,result=result))
Path(__file__).with_name('plugin-edits.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_plugin_edits.py',cases=cases,file_cases=file_cases),indent=2)+'\n')
print(len(cases),len(file_cases))
