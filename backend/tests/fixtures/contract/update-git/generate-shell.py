"""Captura o aviso de reabertura do Electron legado em repositórios isolados."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();reference=a.reference.resolve();output=a.output.resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(reference/'backend'))
 from app import atualizar as u
 rows=[]
 for name in ['absent','unchanged','test-only','source','bad-revision','empty-revision']:
  root=Path(tmp)/name;root.mkdir();u.REPO=root
  def git(*args):return subprocess.check_output(['git','-C',str(root),*args],stderr=subprocess.DEVNULL,text=True).strip()
  git('init','-q');git('config','user.name','Fixture');git('config','user.email','fixture@example.invalid');(root/'shell').mkdir();(root/'shell/main.cjs').write_text('old');git('add','.');git('commit','-qm','base');before=git('rev-parse','HEAD')
  path=root/('shell/main.test.cjs' if name=='test-only' else 'shell/main.cjs');path.write_text('new');git('add','.');git('commit','-qm','next');after=git('rev-parse','HEAD')
  if name!='absent':(root/'shell/node_modules/electron').mkdir(parents=True)
  if name=='unchanged':before=after
  if name=='bad-revision':before='missing'
  if name=='empty-revision':before=''
  rows.append(dict(name=name,result=u._shell_mudou(before,after)))
 output.write_text(json.dumps(dict(source_commit=commit,command='python backend/tests/fixtures/contract/update-git/generate-shell.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/shell-cases.json',cases=rows),ensure_ascii=False,indent=2)+'\n');print(rows)
