"""Captura fatos do painel após mudar a fonte de configuração, com métricas isoladas."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
from types import SimpleNamespace
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(reference/'backend'))
 from app import migration_status as m,git_ops,rust_server
 path=Path(tmp)/'.env';m.settings.update_branch='fixture-branch'
 m._mode=lambda:'rust';m.reason=lambda:None;m._usage=lambda pid:{'rss_bytes':100,'swap_bytes':0,'cpu_percent':None};m._canos_usage=lambda:{'count':0,'rss_bytes':0,'swap_bytes':0,'cpu_percent':0.0}
 m.diag.VERSAO_EM_EXECUCAO='fixture-version';rust_server.child=lambda:None;rust_server.current_binary=lambda:None
 m.time.monotonic=lambda:100;git_ops._run=lambda *a,**kw:SimpleNamespace(returncode=0,stdout='main\n')
 for name in ['valid','absent','invalid-utf8','package']:
  m._branch=None;m._branch_failed_at=0.0
  m.diag.install_manifest=lambda:{'version':'1','commit':'fixture','channel':'beta'} if name=='package' else None
  if name=='absent':path.unlink(missing_ok=True)
  else:path.write_bytes(b'CP_UPDATE_BRANCH=fixture-branch\n' if name=='valid' else b'\xff')
  rows.append(dict(name=name,file_hex=path.read_bytes().hex() if path.exists() else None,facts=m.facts()))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-migration-source.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n')
 print(f'{len(rows)} casos da fonte dos fatos capturados')
