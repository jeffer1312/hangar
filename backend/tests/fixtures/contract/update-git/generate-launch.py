"""Captura lançamento e transferência do lock sem criar processos ou tocar serviços."""
import argparse,ast,json,os,subprocess,tempfile,types,uuid
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
tree=ast.parse((a.reference/'backend/app/atualizar.py').read_text())
names={'_tomar_a_vez','_soltar_a_vez','_kw_destacado','iniciar','reiniciar_agora'}
nodes=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in names]
rows=[]
with tempfile.TemporaryDirectory() as temporary:
 for name in ['update','restart','manual-restart','channel-changed','live-lock','dead-lock','spawn-failed','restart-spawn-failed','transfer-failed','child-progress']:
  folder=Path(temporary)/name;folder.mkdir();(folder/'backend').mkdir();state={};events=[]
  def write(**values):state.update(values);events.append(['state',values.copy()])
  def spawn(args,**kwargs):
   events.append(['spawn',{'restart':'--reiniciar' in args,'state_before_spawn':state.copy(),'stdin_null':kwargs['stdin']==subprocess.DEVNULL,'stdout_null':kwargs['stdout']==subprocess.DEVNULL,'stderr_null':kwargs['stderr']==subprocess.DEVNULL,'detached':kwargs.get('start_new_session')}])
   if 'spawn-failed' in name:raise OSError('fixture-spawn-failed')
   if name=='child-progress':write(etapa='resguardar',passo=1,texto='Guardando')
   return types.SimpleNamespace(pid=424242)
  def replace(source,target):
   if name=='transfer-failed':raise OSError('fixture-transfer-failed')
   os.replace(source,target)
  ns={'os':os,'Path':Path,'subprocess':types.SimpleNamespace(Popen=spawn,DEVNULL=subprocess.DEVNULL),'sys':types.SimpleNamespace(executable='<PYTHON>'),'uuid':types.SimpleNamespace(uuid4=lambda:types.SimpleNamespace(hex='fixture-request')),'_E_WINDOWS':False,'_FLAGS_MOTOR_WINDOWS':0x200|0x08000000,'_base':lambda:folder,'_vivo':lambda pid:pid in [os.getpid(),424242],'_topologia':lambda:'manual' if name=='manual-restart' else 'systemd','alvo':lambda:'beta' if name=='channel-changed' else 'main','_escrever':write,'ETAPAS':[0]*6,'ETAPAS_REINICIO':[0]*2,'REPO':folder,'_BACKEND_PID_ENV':'HANGAR_UPDATE_BACKEND_PID','_RESTART_REQUEST_ENV':'HANGAR_RESTART_REQUEST_ID','tmux':types.SimpleNamespace(_scope_prefix=lambda:['fixture-scope']),'atomico':types.SimpleNamespace(substituir=replace),'log_paths':types.SimpleNamespace(base=lambda:folder/'logs'),'diag':types.SimpleNamespace(registrar=lambda *args,**kwargs:None,erro_campos=lambda e:{}),'_log':types.SimpleNamespace(warning=lambda *args:events.append(['warning',*args[:1],str(args[1])]))}
  exec(compile(ast.Module(body=nodes,type_ignores=[]),'<original-launch>','exec'),ns)
  if name.endswith('lock'):(folder/'rodando.lock').write_text('424242' if name=='live-lock' else '999999999')
  try:result={'value':ns['reiniciar_agora' if 'restart' in name else 'iniciar'](8765,**({'expected_branch':'main'} if name=='channel-changed' else {}))}
  except Exception as error:result={'exception':type(error).__name__,'message':str(error)}
  lock=folder/'rodando.lock';owner=lock.read_text() if lock.exists() else None
  if owner==str(os.getpid()):owner='<PARENT>'
  rows.append({'name':name,'result':result,'state':state,'lock':owner,'events':events})
a.output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(a.reference),'rev-parse','HEAD'],text=True).strip(),'command':'python generate-launch.py --reference <checkout-Python> --output <fixture>','scope':'Funções originais e lock real temporário; Popen, estado e diário injetados; nonce fixo.','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows))
