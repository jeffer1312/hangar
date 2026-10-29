"""Captura o cache da branch mostrado pelo painel de migração, sem processos reais."""
import os,sys,tempfile,json,subprocess
from pathlib import Path
from types import SimpleNamespace
root=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(root/'backend'))
 from app import migration_status as m,git_ops
 m.diag.registrar=lambda *a,**k:None
 rows=[]
 for name,manifest,responses,times in [('package',{'channel':'beta'},[],[100,110]),('frozen',None,[(0,'topic\n'),(0,'changed\n')],[100,110]),('failure-retry',None,[(1,''),(0,'main\n')],[100,130,161]),('empty-retry',None,[(0,''),(0,'main\n')],[100,101])]:
  m._branch=None;m._branch_failed_at=0.0;m.diag.install_manifest=lambda:manifest
  at=[0];calls=[];outputs=list(responses)
  def run(*args,**kw):calls.append(dict(args=args[1:],timeout=kw['timeout']));code,text=outputs.pop(0);return SimpleNamespace(returncode=code,stdout=text)
  git_ops._run=run;m.time.monotonic=lambda:at[0]
  values=[]
  for timestamp in times:at[0]=timestamp;values.append(m._checkout_branch())
  rows.append(dict(name=name,manifest=manifest,responses=responses,times=times,values=values,calls=calls))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-migration-branch.py <checkout-Python> <fixture>',cases=rows),indent=2)+'\n')
 print(f'{len(rows)} casos de branch do painel capturados')
