"""Captura a seleção do commit com árvores git reais e transporte sem rede."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-pin-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_AUTHOR_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.invalid',GIT_COMMITTER_NAME='Fixture',GIT_COMMITTER_EMAIL='fixture@example.invalid');os.chdir(root);sys.path.insert(0,str(ref/'backend'))
 from app import atualizar as app
 app.config.settings.rust_server=True;app.rust_release.platform_key=lambda:'linux-x86_64';os.environ.pop('HANGAR_SERVER_RELEASE_URL',None)
 rows=[]
 for scenario in ['no-workflow','unchanged-build','pending-build','reverted-build','platform-entry','invalid-commit','missing-platform','disabled']:
  repo=root/scenario;repo.mkdir();app.REPO=repo
  def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],stderr=subprocess.DEVNULL,text=True).strip()
  git('init','-b','main')
  def write(path,text):
   file=repo/path;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(text);git('add',path);git('commit','-m','fixture');return git('rev-parse','HEAD')
  first=write('backend/app/rust_server.py','RUST_SERVER_PROTOCOL = 35\n')
  if scenario!='no-workflow':write('.github/workflows/server.yml','on:\n  push:\n    paths:\n      - "crates/**"\n      - "backend/app/rust_server.py"\n  workflow_dispatch:\n')
  published=write('crates/server.rs','published\n');same=write('note.txt','one\n');pending=write('crates/server.rs','pending\n');top=write('note.txt','two\n')
  if scenario=='reverted-build':top=write('crates/server.rs','published\n')
  git('update-ref','refs/remotes/origin/main',top)
  manifest={'commit':published,'files':{f'linux-x86_64/{n}':{} for n in ['hangar-server','hangar-cano','hangar-auth']}}
  if scenario=='platform-entry':manifest['platforms']={'linux-x86_64':{'commit':pending,'protocol':35}}
  if scenario=='invalid-commit':manifest['commit']='--wrong'
  if scenario=='missing-platform':manifest['platforms']={}
  app.config.settings.rust_server=scenario!='disabled';app.rust_release.read_manifest=lambda url:manifest
  selected,release=app.pinned_target('main')
  labels={first:'first',published:'published',same:'same',pending:'pending',top:'top','origin/main':'origin/main',None:None}
  rows.append({'name':scenario,'selected':labels[selected],'release':release is not None,'protocol':app._protocol_at('HEAD'),'newest_35':app._newest_with_protocol('origin/main',35)})
  rows[-1]['newest_35']=labels[rows[-1]['newest_35']]
 out.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-pinning.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/pinning-cases.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} cenários capturados')
