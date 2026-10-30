"""Captura a reconciliação de dono original com estado e trava temporários."""
import argparse,ast,datetime,json,logging,os,subprocess,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d';source=subprocess.check_output(['git','-C',str(ref),'show',f'{commit}:backend/app/atualizar.py'],text=True);tree=ast.parse(source);node=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='estado_para_tela');rows=[]
with tempfile.TemporaryDirectory() as scratch:
 base=Path(scratch);file=base/'estado.json';lock=base/'rodando.lock'
 def read():return json.loads(file.read_text())
 def write(**values):current=read();current.update(values);current['ts']='updated';file.write_text(json.dumps(current))
 for name in ['dead-owner','recent-no-pid','old-no-pid','live-owner']:
  state={'fase':'rodando','passo':2,'total':5,'ts':'recent' if name=='recent-no-pid' else 'old'}
  if name.endswith('owner'):state['pid']=os.getpid() if name=='live-owner' else 999999999
  file.write_text(json.dumps(state));ns={'estado':read,'_base':lambda:base,'_vivo':lambda pid:pid==os.getpid(),'_recente':lambda ts:ts=='recent','_restarted_into_target':lambda state:False,'_log':logging.getLogger('fixture'),'_escrever':write,'_soltar_a_vez':lambda:lock.unlink(missing_ok=True)}
  exec(compile(ast.Module(body=[node],type_ignores=[]),'<original-screen>','exec'),ns);result=ns['estado_para_tela']();stored=read()
  for value in [state,result,stored]:
   if value.get('pid')==os.getpid():value['pid']='<SELF>'
  rows.append({'name':name,'input':state,'result':result,'stored':stored,'lock_exists':lock.exists()})
out.write_text(json.dumps({'source_commit':commit,'command':'python generate-screen-state.py --reference <checkout> --output screen-state.json','scope':'estado_para_tela original por AST; estado real em disco, decisões vivo/recente/reinício injetadas explicitamente.','cases':rows},ensure_ascii=False,indent=2)+'\n')
