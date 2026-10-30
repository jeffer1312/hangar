"""Captura o diagnóstico técnico original nos dois ramos de falha da integração."""
import asyncio,io,json,os,subprocess,sys,tarfile,tempfile,types
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5];COMMIT='8a3ce81c1';cases=[]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT))) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_integracao as original
 original.sincronizacao_ligada=lambda:True;original.memoria_ligada=lambda:False
 logs=[]
 def diagnostic(level,*args,**kwargs):
  error=sys.exception();logs.append({'level':level,'message':args[0],'kind':type(error).__name__,'cause':str(error)})
 original._log=types.SimpleNamespace(info=lambda *a,**k:None,warning=lambda *a,**k:diagnostic('warning',*a,**k),exception=lambda *a,**k:diagnostic('error',*a,**k))
 async def capture(name,force):
  with tempfile.TemporaryDirectory() as folder:
   root=Path(folder);home=root/'home';account=home/'.codex';account.mkdir(parents=True)
   if name!='missing-source':(home/'.claude').mkdir();(home/'.claude/settings.json').write_text('{}')
   logs.clear();events=[];record={}; clock=original.time.time;original.time.time=lambda:100.0
   class Native:
    def __init__(self,*args):pass
    async def __aenter__(self):events.append(['open']);return self
    async def __aexit__(self,*args):events.append(['close'])
   service=original.IntegracaoCodex(home=home,codex_home=account,nativo=Native)
   def instruction(force=False):
    events.append(['instructions',force])
    if name=='instructions-error':raise ValueError('causa técnica token=segredo-sintetico')
    if name=='unexpected-error':raise TypeError('causa técnica token=segredo-sintetico')
   service._instrucoes=instruction;service._migrar_ponte_antiga=lambda:events.append(['legacy']);service._hooks=lambda source,registry:events.append(['hooks'])
   async def plugins(native,desired,registry,force):
    events.append(['plugins',force])
    if name=='plugins-error':raise RuntimeError('fixture')
   service._plugins=plugins
   async def config(*args,**kwargs):events.append(['config'])
   service._config=config
   async def fragments(*args):
    events.append(['fragments'])
    if name=='fragments-error':raise ValueError('fixture')
    if name=='cancelled':raise asyncio.CancelledError()
   service._fragmentos=fragments;service._hooks_arquivos=lambda:events.append(['files']);service._skills=lambda registry:events.append(['skills'])
   async def trust(native):events.append(['trust'])
   service._conferir_confianca=trust
   def fingerprints(*,fontes=False,registry=None,only=None):return {phase:('source-' if fontes else 'result-')+phase for phase in ['instructions','plugins','fragments','skills'] if only in (None,phase)}
   service._phase_fingerprints=fingerprints
   if name=='cached':
    service.raiz.mkdir(parents=True);(service.raiz/'estado.json').write_text(json.dumps({'phases':{phase:{'fingerprint':'result-'+phase} for phase in ['instructions','plugins','fragments','skills']},'marketplaces_em':100}))
   try:
    try:result=await service.reconciliar('fixture',force);error=None
    except asyncio.CancelledError:result=service.status();error='CancelledError'
    if (service.raiz/'estado.json').exists():record=json.loads((service.raiz/'estado.json').read_text())
    return {'name':name,'state':result['estado'],'logs':list(logs)}
   finally:original.time.time=clock
 async def run():
  for name in ['instructions-error','unexpected-error']:
   cases.append(await capture(name,False))

 asyncio.run(run())
Path(__file__).with_name('pipeline-diagnostics.json').write_text(json.dumps({'source_commit':COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_pipeline_diagnostics.py','boundary':'Coordenador e fases originais; operações dos provedores controladas; checkpoint e lock reais.','cases':cases},ensure_ascii=True,indent=2)+'\n');print(len(cases))
