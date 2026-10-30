"""Captura os testes originais de conclusão após a morte do atualizador no reinício."""
import argparse,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
with tempfile.TemporaryDirectory() as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='');os.chdir(base)
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py']))) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;rows=[]
 for name in ['test_motor_morto_pelo_proprio_reinicio_conclui_quando_o_servidor_e_o_novo','test_motor_morto_no_reinicio_sem_servidor_novo_ou_commit_novo_e_falha']:
  for stage in ['reiniciar','nativo']:
   for marker in ([None,'{"sem_build": true}','{"version": "0.1.0.1"}'] if name.startswith('test_motor_morto_pelo') else [None]):
    for motor in [False,True]:
     folder=base/str(len(rows));folder.mkdir();calls=[]
     with pytest.MonkeyPatch.context() as patch:
      repo=tests.repo.__wrapped__(folder,patch);patch.setattr(app,'_SOU_O_MOTOR',motor);head=tests._rev(repo,'HEAD');original=app.estado_para_tela
      def normalize(value):
       value=json.loads(json.dumps(value))
       if isinstance(value.get('log'),list):value['log']=[line.replace(str(repo),'<REPO>').replace(head,'<HEAD>') for line in value['log']]
       if 'ts' in value:value['ts']='<TS>'
       if value.get('pid'):value['pid']='<DEAD>'
       if value.get('backend_pid') in [os.getpid(),os.getpid()+1]:value['backend_pid']='<SELF>' if value['backend_pid']==os.getpid() else '<OTHER>'
       if value.get('commit_para')==head:value['commit_para']='<HEAD>'
       return value
      def screen():
       before=normalize(app.estado());result=original();calls.append({'input':before,'result':normalize(result),'stored':normalize(app.estado())});return result
      patch.setattr(app,'estado_para_tela',screen);test=getattr(tests,name);args={'repo':repo,'stage':stage,'motor':motor,'marker':marker,'tmp_path':folder,'monkeypatch':patch};test(**{k:v for k,v in args.items() if k in __import__('inspect').signature(test).parameters});rows.append({'name':name,'stage':stage,'motor':motor,'marker':marker,'calls':calls})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-screen-restart.py --reference <checkout> --output screen-restart.json','normalization':['ts','pid morto','pid do backend atual/outro','SHA HEAD','caminho temporário do repo no log'],'cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows),'cenários originais passaram')
