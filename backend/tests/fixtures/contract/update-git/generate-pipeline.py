"""Captura ordem e compensação do motor em repositórios temporários, sem serviços."""
import argparse,json,os,shutil,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-pipeline-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_AUTHOR_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.invalid',GIT_COMMITTER_NAME='Fixture',GIT_COMMITTER_EMAIL='fixture@example.invalid');os.chdir(root);sys.path.insert(0,str(ref/'backend'));from app import atualizar as app
 app._topologia=lambda:'manual';app._falta=lambda *args:[];app.config.settings.rust_server=False;app.config.settings.update_branch='';app.rust_release.platform_key=lambda:None;app._subiu=lambda *args,**kwargs:True;app._aplicar_passos=lambda:None;app._instalar_nativo=lambda *args:None;app._shell_mudou=lambda *args:False
 rows=[]
 for scenario in ['success','prepare-failure','rollback-prepare-failure','work-branch','missing-target','restart-failure','same-pid']:
  base=root/scenario;base.mkdir();repo=base/'repo';repo.mkdir();remote=base/'remote';remote.mkdir();app.REPO=repo;os.environ['CLAUDE_CONFIG_DIR']=str(base/'claude')
  def git(cwd,*args):return subprocess.check_output(['git','-C',str(cwd),*args],stderr=subprocess.DEVNULL,text=True).strip()
  git(repo,'init','-b','main');(repo/'backend').mkdir();(repo/'backend/.keep').write_text('fixture');(repo/'content').write_text('old');git(repo,'add','.');git(repo,'commit','-m','old');old=git(repo,'rev-parse','HEAD');git(remote,'init','--bare');git(repo,'remote','add','origin',str(remote));git(repo,'push','-u','origin','main');(repo/'content').write_text('new');git(repo,'add','.');git(repo,'commit','-m','new');new=git(repo,'rev-parse','HEAD');git(repo,'push','origin','main');git(repo,'reset','--hard',old)
  dist=repo/'frontend/dist';dist.mkdir(parents=True);(dist/'index.html').write_text('old-screen')
  calls=[]
  app._topologia=lambda:'systemd' if scenario in ['restart-failure','same-pid'] else 'manual'
  app._pid_do_servidor=lambda *args:4242 if scenario=='same-pid' else None
  def restart(topology,port):
   if scenario=='restart-failure' and (repo/'content').read_text()=='new':raise RuntimeError('nao consegui reiniciar o servidor: fixture-restart-failed')
   if topology=='manual':app._escrever(reiniciar_manual=True)
  app._reiniciar=restart
  app.config.settings.update_branch='missing' if scenario=='missing-target' else ''
  if scenario=='work-branch':git(repo,'checkout','-b','feature')
  def prepare(topology,dist=True,release=None):
   calls.append({'dist':dist,'content':(repo/'content').read_text()})
   if scenario=='rollback-prepare-failure' or (scenario=='prepare-failure' and dist):raise RuntimeError('as dependencias do backend nao sincronizaram: fixture-uv-failed')
   if dist:
    shutil.copytree(repo/'frontend/dist',repo/'frontend/.dist-velho');(repo/'frontend/dist/index.html').write_text('new-screen')
  app._preparar=prepare
  state=app.executar(1)
  fields=['fase','ok','voltou','no_ar','reiniciar_manual','shell_mudou','waiting_for_server_build','texto','erro','resgate','avisos']
  rows.append({'name':scenario,'state':{k:state.get(k) for k in fields},'head':'old' if git(repo,'rev-parse','HEAD')==old else 'new','screen':(repo/'frontend/dist/index.html').read_text(),'previous_exists':(repo/'frontend/.dist-velho').exists(),'prepare_calls':calls})
 out.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-pipeline.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/pipeline-cases.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} cenários capturados')
