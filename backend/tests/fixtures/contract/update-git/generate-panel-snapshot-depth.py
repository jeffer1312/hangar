import contextlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
ref=Path(sys.argv[1]).resolve();out=Path(sys.argv[2]).resolve();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
 module=runpy.run_path(str(ref/'scripts/hangar-panel-data'))
 for depth in [100,10000]:
  nested=0
  for _ in range(depth):nested=[nested]
  for name,sessions,projects,peers in [('session-extra',[{'name':'fixture','state':'idle','extra':nested}],[],{}),('session-branch',[{'name':'fixture','state':'idle','branch':nested}],[],{}),('project-extra',[],[{'name':'fixture','extra':nested}],{}),('project-replaced',[],[{'name':'fixture','cwd_real':nested}],{}),('peer-disabled',[],[],{'peer':{'enabled':False,'extra':nested}})]+([('peer-base-nan',[],[],{'peer':{'base_url':float('nan'),'token':'fixture'}}),('peer-token-infinity',[],[],{'peer':{'base_url':'http://127.0.0.1:1','token':float('inf')}})] if depth==100 else []):
   inputs=dict(sessions_json=json.dumps(sessions),projects_json=json.dumps(projects),peers_json=json.dumps(peers))
   overrides={'env':lambda key:'fixture-token','peers':lambda:peers,'local_base':lambda:'http://local','attached_locally':lambda:set(),'fetch':lambda base,token:sessions,'fetch_projects':lambda base,token:(json.loads(inputs['projects_json']),'')}
   results={}
   for mode in ['original','encoder-reference']:
    stdout=io.StringIO()
    try:
     with patch.dict(module['main'].__globals__,overrides),contextlib.redirect_stdout(stdout):
      if mode=='original':rc=module['main']()
      else:
       with patch.object(json,'dump',lambda value,fp,**kwargs:fp.write(json.dumps(value,**kwargs))):rc=module['main']()
     result=dict(rc=rc,stdout=stdout.getvalue())
    except Exception as error:result=dict(exception=type(error).__name__,message=str(error).splitlines()[0],stdout=stdout.getvalue())
    results[mode]=result
   rows.append(dict(name=name,depth=depth,**inputs,results=results))

out.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip(),python=sys.version,cases=rows),ensure_ascii=True,indent=2)+'\n');print(len(rows))
