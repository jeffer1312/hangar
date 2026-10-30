"""Executa as quatro recusas originais e registra suas respostas sem serviço real."""
import base64,argparse,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
with tempfile.TemporaryDirectory() as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='');os.chdir(base)
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py','.github/workflows/ci.yml']))) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;rows=[]
 for name in ['test_chave_do_dist_so_muda_com_as_fontes_da_tela','test_chave_do_dist_muda_com_o_lock_das_dependencias','test_chave_do_dist_e_a_mesma_conta_do_ci']:
  folder=base/name;folder.mkdir();seen=[]
  with pytest.MonkeyPatch.context() as patch:
   repo=tests.repo.__wrapped__(folder,patch);original=app._chave_da_tela
   def key(revision):
    result=original(revision);files={str(path.relative_to(repo)):base64.b64encode(path.read_bytes()).decode() for path in repo.rglob('*') if path.is_file() and '.git' not in path.relative_to(repo).parts};seen.append({'key':result,'files':files});return result
   patch.setattr(app,'_chave_da_tela',key);getattr(tests,name)(repo);rows.append({'name':name,'calls':seen})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-dist-key.py --reference <checkout> --output dist-key.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows),'testes originais passaram')
