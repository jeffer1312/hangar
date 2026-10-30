"""Executa as asserções originais de canal e captura bytes e referências antes e depois."""
import argparse,base64,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
checkout_ignore=subprocess.check_output(['git','-C',str(ref),'show',commit+':.gitignore']).decode()
names=['test_branch_configurada_alinha_o_checkout_com_ela','test_branch_que_o_origin_nao_tem_falha_sem_tocar_em_nada','test_na_branch_configurada_atualiza_sem_recusa','test_outra_branch_de_trabalho_segue_recusada','test_campo_esvaziado_volta_pra_main_com_resgate','test_branch_local_do_alvo_com_commit_proprio_ganha_resgate','test_branch_configurada_sem_upstream_nao_perde_commit_local','test_resgate_da_branch_local_aparece_nos_avisos']
with tempfile.TemporaryDirectory(prefix='channel-reference-') as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='',CP_RUST_SERVER='0',GIT_CONFIG_GLOBAL=os.devnull,GIT_CONFIG_NOSYSTEM='1');os.chdir(base)
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py']))) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;restart=app._reiniciar;rows=[]
 for name in names:
  folder=base/name;folder.mkdir()
  with pytest.MonkeyPatch.context() as patch:
   repo=tests.repo.__wrapped__(folder,patch);tests._commit(repo,'.gitignore',checkout_ignore)
   if name!='test_branch_configurada_alinha_o_checkout_com_ela':
    (repo/'backend').mkdir(exist_ok=True);(repo/'backend/.env').write_bytes(b'# fixture preservada\r\n')
   canal=tests.canal.__wrapped__(repo,folder,patch);assert tests._git(repo,'fetch','origin').returncode==0;patch.setattr(app,'_topologia',lambda:'manual');patch.setattr(app,'_reiniciar',restart);original=app.executar
   def snapshot():
    files={str(path.relative_to(repo)):base64.b64encode(path.read_bytes()).decode() for path in repo.rglob('*') if path.is_file() and '.git' not in path.relative_to(repo).parts}
    marker=app._caminho_canal();refs=tests._git(repo,'for-each-ref','--format=%(refname) %(objectname)','refs/heads','refs/stash','refs/remotes').stdout.splitlines()
    return {'config':tests._git(repo,'config','--local','--get-regexp','^branch\\.').stdout.splitlines(),'files':files,'refs':refs,'head':tests._rev(repo,'HEAD'),'branch':tests._branch(repo),'marker':base64.b64encode(marker.read_bytes()).decode() if marker.is_file() else None,'stash':tests._git(repo,'stash','list','--format=%gd').stdout.splitlines()}
   def execute(*args,**kwargs):
    bundles={}
    for key,path in [('repo',repo),('origin',folder/'origem.git')]:
     target=folder/(key+'.bundle');target.unlink(missing_ok=True);result=tests._git(path,'bundle','create',str(target),'--all');assert result.returncode==0,result.stderr;bundles[key]=base64.b64encode(target.read_bytes()).decode()
    before=snapshot();target=app.config.settings.update_branch;result=original(*args,**kwargs);rows.append({'name':name,'target':target,'bundles':bundles,'before':before,'after':snapshot(),'state':{key:result.get(key) for key in ['fase','ok','erro','avisos','resgate','reiniciar_manual']}});return result
   patch.setattr(app,'executar',execute);test=getattr(tests,name);args={'repo':repo,'canal':canal,'monkeypatch':patch};test(**{k:v for k,v in args.items() if k in __import__('inspect').signature(test).parameters})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-channel-pipeline.py --reference <checkout> --output channel-pipeline.json','setup':'Fixture original acrescida do .gitignore original versionado, .env sintético em sete cenários e fetch inicial para tornar origin/teste disponível antes de checkout --no-track; refs/remotes e branch.* capturados integralmente.','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows),'execuções; oito testes originais passaram')
