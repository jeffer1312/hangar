import os,sys,tempfile,json,subprocess
from pathlib import Path
import argparse
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args()
reference=args.reference.resolve();out=args.output.resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(reference/'backend'))
 from app import rust_release as r
 r.diag.registrar=lambda *a,**k:None;r.platform_key=lambda:'linux-x86_64';r.release_tag=lambda:'server-beta'
 rows=[]
 for name,data in [('branch-invalid-json',b'{'),('branch-list',b'[]')]:
  urls=[]
  def get(url,deadline):urls.append(url);return data
  r._get=get;dest=Path(tmp)/name
  result=r.fetch(dest=dest)
  rows.append(dict(name=name,result=result,urls=urls,files=list(dest.glob('*'))))
 out.write_text(json.dumps(dict(source_commit=commit,command='python generate-bootstrap-extra.py --reference <checkout-Python> --output bootstrap-extra.json',cases=rows),ensure_ascii=False,indent=2)+'\n')
 print(json.dumps(rows,ensure_ascii=False))
