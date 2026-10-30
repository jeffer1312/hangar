"""Captura build local e restauração da tela com Git temporário e npm injetado."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();output=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-local-dist-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_AUTHOR_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.invalid',GIT_COMMITTER_NAME='Fixture',GIT_COMMITTER_EMAIL='fixture@example.invalid');os.chdir(root);sys.path.insert(0,str(ref/'backend'));from app import atualizar as app
 rows=[];run_original=app._rodar
 for name in ['success','build-failed','incomplete','missing-npm','dirty-main']:
  repo=root/name;repo.mkdir();app.REPO=repo;dist=repo/'frontend/dist';dist.mkdir(parents=True);(dist/'index.html').write_text('old-screen');(repo/'frontend/source.ts').write_text('old-source');(repo/'.gitignore').write_text('frontend/dist/\nfrontend/.dist-velho/\n')
  def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],stderr=subprocess.DEVNULL,text=True).strip()
  git('init','-b','main' if name=='dirty-main' else 'beta');git('add','.');git('commit','-m','fixture')
  if name=='dirty-main':(repo/'frontend/source.ts').write_text('changed-source')
  calls=[]
  def run(args,**kw):
   if args[0]!='/fixture/npm':return run_original(args,**kw)
   calls.append({'args':['npm',*args[1:]],'timeout':kw.get('timeout'),'cwd':'repo' if kw.get('cwd')==repo else None})
   if name=='incomplete':(dist/'index.html').unlink()
   else:(dist/'index.html').write_text('new-screen')
   return subprocess.CompletedProcess(args,1 if name=='build-failed' else 0,'','fixture-build-failed' if name=='build-failed' else '')
  app._rodar=run;app.shutil.which=lambda program:None if name=='missing-npm' else '/fixture/'+program
  try:result={'warning':app._atualizar_dist()}
  except Exception as error:result={'error_type':type(error).__name__,'message':str(error)}
  old=repo/'frontend/.dist-velho';read=lambda path:path.read_text() if path.is_file() else None
  before={'screen':read(dist/'index.html'),'previous':read(old/'index.html'),'previous_exists':old.exists()}
  app._dist_velho_voltar();after={'screen':read(dist/'index.html'),'previous_exists':old.exists()}
  rows.append({'name':name,'calls':calls,'result':result,'before_restore':before,'after_restore':after})
 output.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-local-dist.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/local-dist.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} casos capturados')
