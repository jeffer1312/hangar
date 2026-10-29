"""Captura a versão original usando git real e somente diretórios temporários."""
import json, os, subprocess, sys, tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 root=Path(tmp);os.environ.update(HOME=tmp,USERPROFILE=tmp,GIT_CONFIG_NOSYSTEM='1');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from app import diag
 rows=[]
 for name in ['outside-git','version-only-on-disk']:
  checkout=root/name;checkout.mkdir();diag._ROOT=checkout
  if name=='version-only-on-disk':
   def git(*args):subprocess.run(['git','-C',str(checkout),*args],check=True,capture_output=True)
   git('init','-q');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--allow-empty','-qm','fixture')
   (checkout/'VERSION').write_text('2.3.4\n')
  rows.append(dict(name=name,describe=diag._git_describe() if name=='outside-git' else '<COMMIT>',readable=diag.versao_legivel()))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-boot-real.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=False,indent=2)+'\n')
print(f'{len(rows)} capturas com git real')
