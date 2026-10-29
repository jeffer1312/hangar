"""Captura o modo da cópia anterior em Unix sem tocar a instalação."""
import json,os,stat,subprocess,sys,tempfile
from pathlib import Path
root=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(root/'backend'))
 from app import rust_release
 target=Path(tmp)/'hangar-server';target.write_bytes(b'old');target.chmod(0o750)
 previous_mask=os.umask(0o022)
 try:
  warning=rust_release._keep_previous(target,'hangar-server')
  mode=stat.S_IMODE(target.with_name('hangar-server.prev').stat().st_mode)
 finally:os.umask(previous_mask)
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-backup-mode.py <checkout-Python> <fixture>',source_mode=0o750,umask=0o022,previous_mode=mode,warning=warning),indent=2)+'\n')
