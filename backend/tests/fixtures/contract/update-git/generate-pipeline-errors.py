"""Executa as quatro recusas originais e registra suas respostas sem serviço real."""
import argparse,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
with tempfile.TemporaryDirectory() as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='');os.chdir(base)
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py']))) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;rows=[]
 for name in ['test_falha_no_meio_nao_reinicia','test_erro_inesperado_vira_estado_e_nao_traceback','test_a_vez_e_solta_mesmo_quando_falha']:
  folder=base/name;folder.mkdir();seen=[]
  with pytest.MonkeyPatch.context() as patch:
   repo=tests.repo.__wrapped__(folder,patch);patch.setattr(app,'_subiu',lambda *args,**kwargs:False);patch.setattr(app,'_topologia',lambda:'manual')
   for method in ['checar','executar']:
    original=getattr(app,method)
    def wrapped(*args,_original=original,_name=method,**kwargs):
     result=_original(*args,**kwargs);seen.append({'method':_name,'result':{k:result.get(k) for k in (['pode','faltando','branch','branch_de_trabalho'] if _name=='checar' else ['fase','ok','erro','voltou'])}});return result
    patch.setattr(app,method,wrapped)
   test=getattr(tests,name);args={'repo':repo,'monkeypatch':patch};test(**{k:v for k,v in args.items() if k in __import__('inspect').signature(test).parameters});rows.append({'name':name,'calls':seen,'head':tests._rev(repo,'HEAD'),'branch':tests._branch(repo),'invalid_steps':app.estado().get('passos_invalidos')})
 folder=base/'unexpected-after-code';folder.mkdir()
 with pytest.MonkeyPatch.context() as patch:
  repo=tests.repo.__wrapped__(folder,patch);before=tests._rev(repo,'HEAD');events=[]
  patch.setattr(app,'_topologia',lambda:'manual');patch.setattr(app,'_puxar',lambda pre:tests._commit(repo,'new.txt','new'))
  def unexpected():raise Exception('passo com defeito')
  patch.setattr(app,'_aplicar_passos',unexpected);patch.setattr(app,'_preparar',lambda *args,**kwargs:events.append(['prepare',kwargs.get('dist',True)]));patch.setattr(app,'_reiniciar',lambda *args,**kwargs:events.append(['restart']));patch.setattr(app,'_subiu',lambda *args,**kwargs:True)
  result=app.executar();assert result['voltou'] is True and tests._rev(repo,'HEAD')==before
  rows.append({'name':'unexpected-after-code','calls':[{'method':'executar','result':{key:result.get(key) for key in ['fase','ok','erro','voltou']}}],'restored_head':tests._rev(repo,'HEAD')==before,'events':events})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-pipeline-errors.py --reference <checkout> --output step-effects.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print('três testes originais e um cenário de exceção após código novo passaram')
