"""Executa as quatro recusas originais e registra suas respostas sem serviço real."""
import argparse,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
with tempfile.TemporaryDirectory() as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='');os.chdir(base)
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py']))) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;rows=[]
 for name in ['test_passos_rodam_sabendo_que_o_binario_e_da_atualizacao','test_passo_malformado_vai_pro_estado_e_nao_so_pro_log']:
  folder=base/name;folder.mkdir();seen=[]
  with pytest.MonkeyPatch.context() as patch:
   repo=tests.repo.__wrapped__(folder,patch);patch.setattr(app,'_subiu',lambda *args,**kwargs:False);patch.setattr(app,'_topologia',lambda:'manual')
   for method in ['checar','executar']:
    original=getattr(app,method)
    def wrapped(*args,_original=original,_name=method,**kwargs):
     result=_original(*args,**kwargs);seen.append({'method':_name,'result':{k:result.get(k) for k in (['pode','faltando','branch','branch_de_trabalho'] if _name=='checar' else ['ok','erro'])}});return result
    patch.setattr(app,method,wrapped)
   test=getattr(tests,name);args={'repo':repo,'monkeypatch':patch};test(**{k:v for k,v in args.items() if k in __import__('inspect').signature(test).parameters});rows.append({'name':name,'calls':seen,'head':tests._rev(repo,'HEAD'),'branch':tests._branch(repo),'invalid_steps':app.estado().get('passos_invalidos')})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-step-effects.py --reference <checkout> --output step-effects.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows),'testes originais passaram')
