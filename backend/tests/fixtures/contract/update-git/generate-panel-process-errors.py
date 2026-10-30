import contextlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
ref=Path(sys.argv[1]).resolve();out=Path(sys.argv[2]).resolve();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp,PATH=tmp);os.chdir(tmp)
 action=runpy.run_path(str(ref/'scripts/hangar-panel-action'))
 for name in ['run-spawn','terminal-spawn','run-timeout']:
  tmux=Path(tmp)/'tmux';term=Path(tmp)/'kitty'
  tmux.write_text('invalid executable' if name=='run-spawn' else f'#!{sys.executable}\nimport time\ntime.sleep(6)\n' if name=='run-timeout' else '#!/bin/sh\nexit 0\n');tmux.chmod(0o700)
  term.write_text('invalid executable' if name=='terminal-spawn' else '#!/bin/sh\nexit 0\n');term.chmod(0o700)
  stdout=io.StringIO()
  with patch.dict(action['main'].__globals__,{'api':lambda *args,**kwargs:[{'name':'fixture','tmux':'runner'}]}),patch.object(sys,'argv',['hangar-panel-action','fixture','project','open']),contextlib.redirect_stdout(stdout):rc=action['main']()
  rows.append(dict(name=name,rc=rc,stdout=stdout.getvalue()))
# O PATH temporário não pode afetar a consulta da proveniência.
commit=subprocess.check_output(['/usr/bin/git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
out.write_text(json.dumps(dict(source_commit=commit,python=sys.version,cases=rows),ensure_ascii=True,indent=2)+'\n');print(json.dumps(rows))
