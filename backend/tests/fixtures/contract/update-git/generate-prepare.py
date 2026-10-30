"""Captura preparação do checkout sem rede, instalador ou serviço real."""
import argparse, hashlib, json, os, subprocess, sys, tempfile
from pathlib import Path
from types import SimpleNamespace
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();output=a.output.resolve()
commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-prepare-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(root);sys.path.insert(0,str(ref/'backend'));from app import atualizar as app
 rows=[]
 for name in ['same-lock','changed-lock','missing-marker','no-modules','missing-lock','missing-uv','uv-failure','missing-npm','npm-failure','windows-success','windows-npm-failure','invalid-marker-utf8','same-lock-controls','same-lock-unicode']:
  repo=root/name;repo.mkdir();(repo/'backend').mkdir();state=repo/'state';state.mkdir();app.REPO=repo;os.environ['CLAUDE_CONFIG_DIR']=str(repo/'claude');app._base=lambda:state
  lock=b'{}';digest=hashlib.sha256(lock).hexdigest()
  if name!='missing-lock':(repo/'package-lock.json').write_bytes(lock)
  if name!='no-modules':(repo/'node_modules').mkdir()
  if name!='missing-marker':(state/'package-lock.sha').write_bytes(b'\xff' if name=='invalid-marker-utf8' else (digest if name=='same-lock' else 'old').encode())
  if name=='same-lock-controls':(state/'package-lock.sha').write_text('\x1c'+digest+'\x1f')
  if name=='same-lock-unicode':(state/'package-lock.sha').write_text('\u2003'+digest+'\u3000')
  calls=[]
  def run(args,**kw):
   calls.append({'program':Path(args[0]).name,'args':args[1:],'cwd':'backend' if kw.get('cwd')==repo/'backend' else 'repo' if kw.get('cwd')==repo else None,'timeout':kw.get('timeout')})
   fail=(name=='uv-failure' and args[-1]=='sync') or (name in ['npm-failure','windows-npm-failure'] and 'ci' in args)
   return SimpleNamespace(returncode=int(fail),stdout='fixture-failed' if fail else '',stderr='')
  app._rodar=run;app.shutil.which=lambda program:None if name=='missing-'+program else '/fixture/'+program
  app._stop_windows_front=lambda:calls.append({'action':'stop-front'})
  try:app._preparar('windows' if name.startswith('windows') else 'systemd',dist=False);result={'ok':True}
  except Exception as error:result={'error_type':type(error).__name__,'message':str(error)}
  marker=state/'package-lock.sha';rows.append({'name':name,'calls':calls,'result':result,'marker_hex':marker.read_bytes().hex() if marker.exists() else None})
 output.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-prepare.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/prepare.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} casos capturados')
