"""Captura varredura de projetos em pasta temporária, inclusive falha do backend."""
import json, os, runpy, subprocess, sys, tempfile
from pathlib import Path
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
 action=runpy.run_path(str(reference/'scripts/hangar-panel-action'))
 common=sys.modules['hangar_panel_common'];root=Path(tmp)/'projects';root.mkdir()
 for name in ['zeta','Alpha','.hidden']:(root/name).mkdir()
 (root/'Alpha'/'package.json').write_text('{"scripts":{"dev":"vite"}}')
 (root/'loose-file').write_text('ignored')
 for name,path,response in [('missing','',[]),('not-directory',str(root/'loose-file'),[]),('existing',str(root),[{'cwd':str(root/'zeta')}]),('backend-failed',str(root),None)]:
  calls=[]
  def api(*args,**kwargs):
   calls.append(dict(args=args,kwargs=kwargs))
   if response is None:raise common.PanelError('fixture refusal')
   return response
  with patch.dict(action['op_scan_folder'].__globals__,{'api':api}):result=action['op_scan_folder'](path)
  rows.append(dict(name=name,path=path,response=response,calls=calls,result=result))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-panel-scan.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2).replace(tmp,'<ROOT>')+'\n')
 print(f'{len(rows)} casos de varredura capturados')
