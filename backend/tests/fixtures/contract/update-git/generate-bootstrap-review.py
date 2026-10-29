"""Captura destinos especiais e entradas de manifesto inválidas no downloader original."""
import argparse,hashlib,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--platform',default='linux-x86_64');a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(ref/'backend'))
 from app import rust_release as r
 r.diag.registrar=lambda *a,**k:None;r.platform_key=lambda:a.platform;r._E_WINDOWS=a.platform.startswith('windows')
 rows=[]
 for name,entry in [('directory',None),('symlink-directory',None),('entry-array',[]),('entry-list',['invalid']),('entry-string','invalid'),('entry-number',7),('entry-bool',True),('entry-null',None)]:
  if a.platform.startswith('windows') and not name.startswith('entry-'):continue
  root=Path(tmp)/name;dest=root/'dest';dest.mkdir(parents=True);branch=name.startswith('entry-');r.release_tag=lambda:'server-beta' if branch else 'server-latest'
  if name=='directory':(dest/'hangar-server').mkdir()
  if name=='symlink-directory':(dest/'unchanged').mkdir();(dest/'hangar-server').symlink_to('unchanged',target_is_directory=True)
  manifest={'files':{f'{a.platform}/{binary}':{'name':binary,'sha256':hashlib.sha256(('new-'+binary).encode()).hexdigest()} for binary in r.NAMES}}
  urls=[]
  def get(url,deadline):
   urls.append(url)
   if url.endswith('server-latest.json'):
    doc=json.loads(json.dumps(manifest))
    if '/server-beta/' in url:doc['files'][a.platform+'/hangar-server']=entry
    return json.dumps(doc).encode()
   return ('new-'+url.rsplit('/',1)[-1]).encode()
  r._get=get;result=r.fetch(dest=dest)
  files={str(p.relative_to(dest)):p.read_text() for p in dest.rglob('*') if p.is_file()}
  rows.append(dict(name=name,entry=entry,code=1 if result else 0,urls=urls,files=files,directories=sorted(str(p.relative_to(dest)) for p in dest.rglob('*') if p.is_dir()),links=sorted(str(p.relative_to(dest)) for p in dest.rglob('*') if p.is_symlink())))
 out.write_text(json.dumps(dict(source_commit=commit,command=f'python backend/tests/fixtures/contract/update-git/generate-bootstrap-review.py --reference <checkout-Python> --platform {a.platform} --output <fixture>',cases=rows),ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} capturas de publicação e fallback')
