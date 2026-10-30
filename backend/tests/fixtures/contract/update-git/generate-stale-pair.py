"""Captura a conferência original do par instalado sem acessar instalação pessoal."""
import argparse, hashlib, json, os, subprocess, sys, tempfile, types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
with tempfile.TemporaryDirectory() as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(root)
 app=types.ModuleType('app');app.__path__=[];sys.modules['app']=app
 for name in ['atomico','diag']:sys.modules['app.'+name]=types.ModuleType('app.'+name)
 for name in ['rust_bins','rust_release']:
  m=types.ModuleType('app.'+name);m.__file__=str(root/'backend/app'/f'{name}.py');sys.modules[m.__name__]=m
  exec(subprocess.check_output(['git','-C',str(ref),'show',f'{commit}:backend/app/{name}.py'],text=True),m.__dict__)
 release=sys.modules['app.rust_release'];rows=[]
 for platform in ['linux-x86_64','windows-x86_64','macos-aarch64']:
  for mode in ['intact','missing-metadata','corrupt-metadata','wrong-server-hash','wrong-library-hash']:
   dest=root/platform/mode;dest.mkdir(parents=True);ext='.exe' if platform.startswith('windows') else '';files={}
   for name in release.NAMES:
    data=name.encode();sha=hashlib.sha256(data).hexdigest();files[f'{platform}/{name}']={'sha256':sha};target=release._auth_target(dest,platform,sha) if name=='hangar-auth' else dest/(name+ext);target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data)
   manifest={'files':files};lib=release._auth_target(dest,platform,files[f'{platform}/hangar-auth']['sha256']);meta={'abi':1,'store_version':1,'library':lib.relative_to(dest).as_posix(),'library_sha256':files[f'{platform}/hangar-auth']['sha256'],'server_sha256':files[f'{platform}/hangar-server']['sha256']}
   if mode=='wrong-server-hash':meta['server_sha256']='0'*64
   if mode=='wrong-library-hash':meta['library_sha256']='0'*64
   if mode!='missing-metadata':(dest/'hangar-server.auth.json').write_text('{' if mode=='corrupt-metadata' else json.dumps(meta))
   release.platform_key=lambda:platform;release.bin_dir=lambda:dest
   rows.append({'platform':platform,'mode':mode,'manifest':manifest,'files':{str(p.relative_to(dest)):p.read_text() for p in dest.rglob('*') if p.is_file()},'stale':release.stale(('fixture',None,manifest))})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-stale-pair.py --reference <checkout> --output stale-pair.json','cases':rows},indent=2)+'\n')
