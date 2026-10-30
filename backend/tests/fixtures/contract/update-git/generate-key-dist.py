"""Captura download pela chave e fallback local, sem rede ou npm reais."""
import argparse,hashlib,io,json,os,subprocess,sys,tarfile,tempfile,urllib.error,urllib.request,http.client
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();output=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-key-dist-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_AUTHOR_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.invalid',GIT_COMMITTER_NAME='Fixture',GIT_COMMITTER_EMAIL='fixture@example.invalid');os.chdir(root);sys.path.insert(0,str(ref/'backend'));from app import atualizar as app
 run_original=app._rodar;app.config.settings.update_branch='';rows=[]
 for name in ['key-success','key-missing','key-hash','key-truncated','key-cut','key-incomplete']:
  repo=root/name;repo.mkdir();app.REPO=repo
  for folder,file in [('frontend','a.ts'),('packages/core','b.ts'),('messages','en.json'),('project.inlang','settings.json')]:
   (repo/folder).mkdir(parents=True);(repo/folder/file).write_text(file)
  for file in ['package.json','package-lock.json']:(repo/file).write_text('{}')
  (repo/'.gitignore').write_text('frontend/dist/\nfrontend/.dist-velho/\n');dist=repo/'frontend/dist';dist.mkdir();(dist/'index.html').write_text('old-screen')
  def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],stderr=subprocess.DEVNULL,text=True).strip()
  git('init','-b','main');git('add','.');git('commit','-m','old');old=git('rev-parse','HEAD');(repo/'frontend/a.ts').write_text('next');git('add','.');git('commit','-m','new');git('update-ref','refs/remotes/origin/main','HEAD');git('reset','--hard',old)
  archive=io.BytesIO()
  with tarfile.open(fileobj=archive,mode='w:gz') as tar:
   item=tarfile.TarInfo('other.html' if name=='key-incomplete' else 'index.html');item.size=10;tar.addfile(item,io.BytesIO(b'new-screen'))
  data=archive.getvalue() if name!='key-truncated' else b'not a gzip'
  urls=[];commands=[];events=[]
  def urlopen(url,timeout):
   urls.append({'url':url,'timeout':timeout})
   if name=='key-missing':raise urllib.error.HTTPError(url,404,'Not Found',{},None)
   if url.endswith('.sha256'):return io.BytesIO(('bad' if name=='key-hash' else hashlib.sha256(data).hexdigest()).encode())
   if name=='key-cut':raise http.client.IncompleteRead(b'abc',7)
   return io.BytesIO(data)
  def run(args,**kw):
   if args[0]!='/fixture/npm':return run_original(args,**kw)
   commands.append({'args':['npm',*args[1:]],'timeout':kw.get('timeout')});(dist/'index.html').write_text('local-screen');return subprocess.CompletedProcess(args,0,'','')
  urllib.request.urlopen=urlopen;app._rodar=run;app.shutil.which=lambda program:'/fixture/'+program;app.diag.registrar=lambda *args,**kw:events.append({'args':args,'fields':kw})
  result=app._atualizar_dist();previous=repo/'frontend/.dist-velho'
  rows.append({'name':name,'key':app._chave_da_tela('HEAD'),'urls':urls,'commands':commands,'events':events,'warning':result,'screen':(dist/'index.html').read_text(),'previous':(previous/'index.html').read_text() if (previous/'index.html').exists() else None})
 output.write_text(json.dumps({'source_commit':commit,'command':'python generate-key-dist.py --reference <checkout-Python> --output <fixture>','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} casos capturados')
