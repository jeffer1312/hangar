"""Captura a etapa de fragmentos com protocolo nativo e arquivos sintéticos."""
import asyncio,copy,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
scenarios=[dict(name='empty'),dict(name='missing-settings',missing=True),dict(name='memory-empty',memory=True),dict(name='changed-source',changed=True),dict(name='env-complete',settings={'env':{'TOKEN':'fixture'}},native_config='[shell_environment_policy.set]\nTOKEN="fixture"\n'),dict(name='env-incomplete',settings={'env':{'TOKEN':'fixture'}})]
for value in [None,[],False,{'a':3},{'a=b':'x'},{'a\0':'x'},{'a':'x\0'}]:scenarios.append(dict(name='invalid-env-'+str(len(scenarios)),settings={'env':value}))
for value in [None,[],{'one':None},{'one':{}},{'one':{'command':3}},{'one':{'command':'x','args':[1]}},{'one':{'url':'https://fixture.invalid','env':{'a':3}}}]:scenarios.append(dict(name='invalid-mcp-'+str(len(scenarios)),mcp={'mcpServers':value}))
for name in ['ignored','recognized','essential-failure','memory-failure','memory-exception']:
 memory=name.startswith('memory-');kind='MEMORY' if memory else 'SUBAGENTS'
 items=[{'itemType':kind,'details':{'subagents':[{'name':'agent'}] if name!='ignored' else [],'memory':[]}}]
 scenarios.append(dict(name=name,memory=memory,files={'agents/agent.md':'---\nname: agent\n---\nBody'},items=items,failures=name.endswith('failure'),native_error=name.endswith('exception'),native_config='[agents.agent]\nconfig_file="@CODEX@/agents/agent.toml"\n',outputs={'agents/agent.toml':'model = "fixture"\n'}))
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT))) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao as original
 async def run(scenario):
  with tempfile.TemporaryDirectory() as folder:
   base=Path(folder);home=base/'home';account=base/'account';work=base/'work';backups=work/'backups'
   for directory in [home/'.claude',account,work]:directory.mkdir(parents=True)
   if not scenario.get('missing'):(home/'.claude/settings.json').write_text(json.dumps(scenario.get('settings',{})))
   if 'mcp' in scenario:(home/'.claude.json').write_text(json.dumps(scenario['mcp']))
   for name,text in scenario.get('files',{}).items():
    path=home/'.claude'/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(text)
   events=[];state={'avisos':[]};registry={};counter=0
   def clean(value):
    if hasattr(value,'codigo'):return clean(original.serializar(value))
    if isinstance(value,dict):return {clean(k):clean(v) for k,v in value.items()}
    if isinstance(value,(tuple,list,set)):return [clean(v) for v in value]
    if isinstance(value,Path):value=str(value)
    if isinstance(value,str):return value.replace(str(base),'ROOT')
    return value
   def fingerprint(**kwargs):
    nonlocal counter
    counter+=1;events.append(['fingerprint',kwargs]);return 'changed' if scenario.get('changed') and counter>1 else 'same'
   async def history(native):events.append(['history']);return {'mcp_servers':set(),'agents':set(),'commands':set()}
   async def config(native,mcp,agents,**kwargs):events.append(['config',clean(mcp),clean(agents),clean(kwargs)])
   def checkpoint(registry):events.append(['checkpoint',clean(copy.deepcopy(registry))])
   def hooks(value,registry):events.append(['hooks',clean(value)])
   class Importer:
    def __init__(self,stage,codex,binario,**kwargs):self.stage=stage;self.codex=codex;events.append(['open',kwargs])
    async def __aenter__(self):
     events.append(['stage', {str(path.relative_to(self.stage)):path.read_bytes().hex() for path in sorted(self.stage.rglob('*')) if path.is_file()}])
     (self.codex/'config.toml').write_text(scenario.get('native_config','').replace('@CODEX@',str(self.codex)))
     return self
    async def __aexit__(self,*args):events.append(['close'])
    async def detectar(self):events.append(['detect']);return copy.deepcopy(scenario.get('items',[]))
    async def importar(self,items):
     events.append(['import',items])
     if scenario.get('native_error'):raise original.CodexNativoErro('fixture')
     for name,text in scenario.get('outputs',{}).items():
      path=self.codex/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(text)
     return {'itemTypeResults':[{'failures':['fixture'] if scenario.get('failures') else []}]}
   original.memoria_ligada=lambda:scenario.get('memory',False)
   obj=SimpleNamespace(home=home,codex_home=account,raiz=work,backups=backups,binario='fixture',nativo=Importer,_estado=state,_historico=history,fingerprint=fingerprint,_checkpoint=checkpoint,_hooks=hooks,_config=config)
   try:await original.IntegracaoCodex._importar_fragmentos(obj,None,registry);result={'ok':True}
   except Exception as error:result={'error':type(error).__name__,'message':str(error)}
   result.update(events=events,registry=clean(registry),warnings=clean(state['avisos']),files={str(path.relative_to(account)):path.read_bytes().hex() for path in sorted(account.rglob('*')) if path.is_file()})
   return clean(result)
 for scenario in scenarios:cases.append(dict(input=scenario,result=asyncio.run(run(scenario))))
Path(__file__).with_name('stage-flow.json').write_text(json.dumps(dict(source_commit=COMMIT,platform='posix',command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_stage_flow.py',cases=cases),indent=2)+'\n')
print(len(cases))
