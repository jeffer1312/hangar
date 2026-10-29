import os,sys,tempfile,json,subprocess
from pathlib import Path
import argparse
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args()
reference=args.reference.resolve();out=args.output.resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(reference/'backend'))
 from app import rust_release as r
 r.diag.registrar=lambda *a,**k:None
 dest=Path(tmp)/'dest';dest.mkdir();before={'hangar-server.old':'old','HANGAR-SERVER.OLD-3':'old','hangar-cano.oldmore':'old','unrelated.old':'keep','hangar-server.prev':'keep'}
 for n,data in before.items():(dest/n).write_text(data)
 (dest/'hangar-server.old-directory').mkdir()
 for name in r.NAMES:r._sweep_old(dest/name)
 after={p.name:p.read_text() for p in dest.iterdir() if p.is_file()}
 out.write_text(json.dumps(dict(source_commit=commit,command='python generate-bootstrap-old.py --reference <checkout-Python> --output bootstrap-old.json',before=before,after=after,directories=[p.name for p in dest.iterdir() if p.is_dir()]),ensure_ascii=False,indent=2)+'\n');print(after)
