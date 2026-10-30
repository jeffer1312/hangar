"""Captura stdout UTF-8 estrito dos dois comandos do painel."""
import contextlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
 action=runpy.run_path(str(reference/'scripts/hangar-panel-action'));snapshot=runpy.run_path(str(reference/'scripts/hangar-panel-data'))
 for name,value in [('unicode','é'),('literal',chr(0x10ffff)),('surrogate','\ud800'),('surrogate-nested',['\ud800'])]:
  for command in ['action','data']:
   sink=io.BytesIO();stdout=io.TextIOWrapper(sink,encoding='utf-8',errors='strict');result={}
   globals_=action['main'].__globals__ if command=='action' else snapshot['main'].__globals__
   overrides={'api':lambda *args,**kwargs:{'pane':value}} if command=='action' else {'env':lambda key:'fixture-token','peers':lambda:{},'local_base':lambda:'http://local','attached_locally':lambda:set(),'fetch':lambda base,token:[],'fetch_projects':lambda base,token:([{'name':'fixture','extra':value}],'')}
   try:
    with patch.dict(globals_,overrides),patch.object(sys,'argv',['hangar-panel-action','fixture','project','pane']),contextlib.redirect_stdout(stdout):rc=(action if command=='action' else snapshot)['main']()
    result['rc']=rc
   except Exception as error:result.update(rc=1,exception=type(error).__name__,message=str(error).splitlines()[0])
   stdout.flush();result['stdout_hex']=sink.getvalue().hex();rows.append(dict(name=name,command=command,value_json=json.dumps(value),result=result))
output.write_text(json.dumps(dict(source_commit=commit,command='python generate-panel-stdout.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n');print([(r['name'],r['command'],r['result']['rc'],len(r['result']['stdout_hex'])//2) for r in rows])
