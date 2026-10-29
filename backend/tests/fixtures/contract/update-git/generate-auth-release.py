"""Captura o par servidor/biblioteca da main, com publicação e falhas isoladas."""
import argparse,hashlib,json,os,subprocess,sys,tempfile,urllib.error
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--platform',default='linux-x86_64');a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-auth-reference-') as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(ref/'backend'))
 from app import rust_release as r
 r.diag.registrar=lambda *a,**k:None;r.platform_key=lambda:a.platform;r._E_WINDOWS=a.platform.startswith('windows');base='https://release.invalid';r.RELEASES_URL=base
 sha=lambda data:hashlib.sha256(data).hexdigest()
 library='hangar_auth.dll' if a.platform.startswith('windows') else 'libhangar_auth.dylib' if a.platform.startswith('macos') else 'libhangar_auth.so'
 ext='.exe' if a.platform.startswith('windows') else ''
 rows=[];original_install=r._install
 for name in ['new','upgrade','same','hold-new','hold-upgrade','auth-hash-failure','server-hash-failure','auth-download-failure','server-download-failure','cano-download-failure','server-publish-failure','metadata-publish-failure','auth-existing-corrupt','branch-keeps','missing-auth','invalid-auth-sha','branch-missing-auth','metadata-repair','branch-server-download-failure','branch-server-publish-failure','branch-metadata-publish-failure']:
  dest=Path(tmp)/name;dest.mkdir();old=name not in ['new','hold-new','missing-auth','invalid-auth-sha','branch-missing-auth','branch-server-download-failure','branch-server-publish-failure','branch-metadata-publish-failure'];branch=name.startswith('branch-')
  content={n:('new-'+n).encode() for n in r.NAMES}
  def metadata(server,auth):return json.dumps(dict(abi=1,store_version=1,server_sha256=sha(server),library_sha256=sha(auth),library=f'auth/{sha(auth)}/{library}'),sort_keys=True).encode()
  if old:
   prefix='new-' if name in ['same','metadata-repair'] else 'old-';server=(prefix+'hangar-server').encode();auth=(prefix+'hangar-auth').encode();lib=dest/'auth'/sha(auth)/library;lib.parent.mkdir(parents=True);lib.write_bytes(auth);(dest/('hangar-server'+ext)).write_bytes(server);(dest/('hangar-cano'+ext)).write_bytes((prefix+'hangar-cano').encode());(dest/'hangar-server.auth.json').write_bytes(b'{}' if name=='metadata-repair' else metadata(server,auth))
  if name=='auth-existing-corrupt':lib=dest/'auth'/sha(content['hangar-auth'])/library;lib.parent.mkdir(parents=True);lib.write_bytes(b'corrupt')
  manifest={'commit':'b'*40,'files':{f'{a.platform}/{n}':{'name':n,'sha256':sha(data)} for n,data in content.items()}}
  if name=='missing-auth':del manifest['files'][a.platform+'/hangar-auth']
  if name=='invalid-auth-sha':manifest['files'][a.platform+'/hangar-auth']['sha256']='invalid'
  urls=[]
  def get(url,deadline):
   urls.append(url)
   if url.endswith('server-latest.json'):
    if name in ['branch-keeps','branch-server-download-failure','branch-server-publish-failure','branch-metadata-publish-failure'] and '/server-beta/' in url:raise urllib.error.HTTPError(url,404,'missing',{},None)
    doc=json.loads(json.dumps(manifest))
    if name=='branch-missing-auth' and '/server-beta/' in url:del doc['files'][a.platform+'/hangar-auth']
    return json.dumps(doc).encode()
   binary=url.rsplit('/',1)[-1]
   if name.removeprefix('branch-')==binary.replace('hangar-','')+'-download-failure':raise OSError('fixture-download-failed')
   return b'wrong' if name==binary.replace('hangar-','')+'-hash-failure' else content[binary]
  r._get=get;r.release_tag=lambda:'server-beta' if branch else 'server-latest';failed=[]
  def install(data,target):
   if not failed and ((name.removeprefix('branch-')=='server-publish-failure' and target.name=='hangar-server'+ext) or (name.removeprefix('branch-')=='metadata-publish-failure' and target.name=='hangar-server.auth.json')):
    failed.append(True);raise OSError('fixture-publication-failed')
   return original_install(data,target)
  r._install=install
  snapshot=lambda:{str(path.relative_to(dest)).replace('\\','/'):path.read_bytes().decode() for path in sorted(dest.rglob('*')) if path.is_file()}
  hold=[] if name.startswith('hold-') else None;initial=snapshot();result=r.fetch(dest=dest,hold=hold);before=snapshot();published=r.install_held(hold) if hold is not None else None;after=snapshot();restored=r.restore_previous(dest) if hold is not None else None
  clean=lambda value:[v.replace(str(dest),'<DEST>') for v in value] if value is not None else None
  rows.append(dict(name=name,initial=initial,result=clean(result),urls=urls,before_publish=before,publish_warnings=clean(published),after_publish=after,restore_warnings=clean(restored),after_restore=snapshot()))
 out.write_text(json.dumps(dict(source_commit=commit,platform=a.platform,command='python backend/tests/fixtures/contract/update-git/generate-auth-release.py --reference <checkout-Python> --platform '+a.platform+' --output <fixture>',cases=rows),ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} casos capturados de {commit}')
