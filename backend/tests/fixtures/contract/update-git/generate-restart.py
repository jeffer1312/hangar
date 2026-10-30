"""Captura reinício e porta literal sem executar gerenciadores de serviços."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
from types import SimpleNamespace
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();output=a.output.resolve()
commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-restart-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_FRONT_PORT='9911');os.chdir(root);sys.path.insert(0,str(ref/'backend'));from app import atualizar as app
 rows=[]
 cases=[('systemd-front','systemd',None),('systemd-only','systemd',None),('systemd-failed','systemd',None),('manual','manual',None),('launchd','launchd',None),('launchd-failed','launchd',None),('windows','windows','CP_FRONT_PORT=7890\n'),('windows-failed','windows','CP_FRONT_PORT=7890\n'),('windows-absent','windows',None),('windows-empty','windows','CP_FRONT_PORT=\n'),('windows-quotes','windows','CP_FRONT_PORT="7890"\n'),('windows-unicode','windows','CP_FRONT_PORT=７８９０\n'),('windows-negative','windows','CP_FRONT_PORT=-1\n'),('windows-large','windows','CP_FRONT_PORT=70000\n'),('windows-space-key','windows',' CP_FRONT_PORT=7890\n'),('windows-duplicate','windows','CP_FRONT_PORT=7800\nCP_FRONT_PORT=7900\n'),('windows-fraction','windows','CP_FRONT_PORT=7890.0\n')]
 cases += [('windows-plus-zeroes','windows','CP_FRONT_PORT=+00078\n'),('windows-negative-zero','windows','CP_FRONT_PORT=-000\n'),('windows-underscore','windows','CP_FRONT_PORT=7_890\n'),('windows-controls','windows','CP_FRONT_PORT=\x1c7890\x1f\n'),('windows-4300','windows','CP_FRONT_PORT='+'1'*4300+'\n'),('windows-4301','windows','CP_FRONT_PORT='+'1'*4301+'\n')]
 for name,topology,dotenv in cases:
  repo=root/name;repo.mkdir();(repo/'backend').mkdir();state=repo/'state';state.mkdir();app.REPO=repo;app._base=lambda:state
  if dotenv is not None:(repo/'backend/.env').write_text(dotenv,encoding='utf8')
  calls=[]
  def run(args,**kw):
   calls.append({'args':[str(v).replace(str(repo),'<REPO>').replace(f'gui/{os.getuid()}/','gui/<UID>/') for v in args],'timeout':kw.get('timeout')})
   fail=name.endswith('-failed') or (name=='systemd-only' and 'list-unit-files' in args)
   return SimpleNamespace(returncode=int(fail),stdout='fixture-failed' if fail else '',stderr='')
  app._rodar=run
  try:app._reiniciar(topology,8123);result={'ok':True}
  except Exception as error:result={'error_type':type(error).__name__,'message':str(error)}
  value=app.estado();value.pop('ts',None)
  rows.append({'name':name,'topology':topology,'dotenv':dotenv,'calls':calls,'result':result,'state':value})
 output.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-restart.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/restart.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} casos capturados')
