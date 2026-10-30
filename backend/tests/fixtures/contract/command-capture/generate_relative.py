"""Captura descoberta e execução com PATH relativo, cwd distinto e ENOEXEC."""
import json, os, shutil, subprocess, tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
cases=[]
for search in ('bin','.','', ':', 'bin:', 'absolute'):
 for mode in ('text','script-name','binary-name'):
  for shebang in ((True,) if mode=='binary-name' else (True,False)):
   with tempfile.TemporaryDirectory() as raw:
    root=Path(raw);(root/'bin').mkdir();(root/'plugin/bin').mkdir(parents=True)
    for path,text in [(root/'bin/node','outer'),(root/'node','outer'),(root/'plugin/bin/node','inner'),(root/'plugin/node','inner')]:
     if mode=='binary-name':path.symlink_to('/usr/bin/python3')
     else:
      body='printf %s "$0"' if mode=='script-name' else f'printf {text}'
      path.write_text(('#!/bin/sh\n' if shebang else '')+body+'\n');path.chmod(0o700)
    env={'PATH':str(root/'bin') if search=='absolute' else search}
    args=['-c',"import sys;print(sys.orig_argv[0],end='')"] if mode=='binary-name' else []
    previous=Path.cwd()
    try:
     os.chdir(root);program=shutil.which('node',path=env['PATH'])
     try:
      if program is None:result=dict(absent=True)
      else:
       output=subprocess.run([program,*args],cwd=root/'plugin',env=env,capture_output=True,timeout=2)
       result=dict(success=output.returncode==0,stdout=output.stdout.decode(),stderr=output.stderr.decode())
     except OSError as error:result=dict(error=type(error).__name__)
    finally:os.chdir(previous)
    cases.append(json.loads(json.dumps(dict(search=search,mode=mode,shebang=shebang,args=args,program=program,result=result)).replace(raw,'$ROOT')))
for layout in ({}, {'node':'denied'}, {'node':'invalid'}, {'node':'denied','bin/node':'valid'}, {'node':'invalid','bin/node':'valid'}, {'bin/node':'invalid'}, {'node':'directory','bin/node':'valid'}, {'node':'invalid','bin/node':'denied'}):
 with tempfile.TemporaryDirectory() as raw:
  root=Path(raw);(root/'plugin/bin').mkdir(parents=True)
  for rel,kind in layout.items():
   path=root/'plugin'/rel
   if kind=='directory':path.mkdir()
   else:
    path.write_text(('' if kind=='invalid' else '#!/bin/sh\n')+'printf allowed\n');path.chmod(0o600 if kind=='denied' else 0o700)
  try:
   output=subprocess.run(['node'],cwd=root/'plugin',env={'PATH':':bin'},capture_output=True,timeout=2)
   result=dict(success=output.returncode==0,stdout=output.stdout.decode(),stderr=output.stderr.decode())
  except OSError as error:result=dict(error=type(error).__name__)
  cases.append(dict(search=':bin',mode='errno',shebang=True,args=[],direct=True,layout=layout,program='node',result=result))
Path(__file__).with_name('relative.json').write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),command='cd backend && uv run python tests/fixtures/contract/command-capture/generate_relative.py',cases=cases),indent=2)+'\n')
print(len(cases))
