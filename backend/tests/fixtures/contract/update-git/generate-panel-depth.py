import contextlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
ref=Path(sys.argv[1]).resolve();output_path=Path(sys.argv[2]).resolve();rows=[];dump=json.dumps
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
 action=runpy.run_path(str(ref/'scripts/hangar-panel-action'));error_type=sys.modules['hangar_panel_common'].PanelError
 for depth in [100,1500,10000]:
  nested=0
  for _ in range(depth):nested=[nested]
  for name,args,responses in [('pane',['fixture','project','pane'],[{'pane':nested}]),('kill',['fixture','kill'],[{'ignored':nested}]),('start',['fixture','project','start'],[{'state':'working','extra':nested}]),('info',['fixture','info'],[[{'name':'fixture','branch':nested}],[]]),('info-error',['fixture','info'],[[{'name':'fixture','branch':nested}],error_type('not a git repository')]),('commit',['fixture','commit'],[[{'path':nested}],{}]),('import',['fixture','import-candidates'],[[{'name':nested,'cwd':'/remote/missing','command':'','port':nested}]])]:
   results={}
   for encoding in ['default','capture-result']:
    pending=list(responses);out=io.StringIO();captured=[]
    def api(*args,**kwargs):
     response=pending.pop(0)
     if isinstance(response,Exception):raise response
     return response
    def encode(value,*args,**kwargs):captured.append(value);return 'null'
    try:
     with patch.dict(action['main'].__globals__,{'api':api}),patch.object(sys,'argv',['hangar-panel-action',*args]),contextlib.redirect_stdout(out):
      if encoding=='capture-result':
       with patch.object(json,'dumps',encode):rc=action['main']()
      else:rc=action['main']()
     results[encoding]={'rc':rc,'stdout':out.getvalue(), 'stdout_bytes':len(out.getvalue().encode())}
    except Exception as error:results[encoding]={'exception':type(error).__name__,'message':str(error)}
   rows.append({'name':name,'depth':depth,'results':results})
output_path.write_text(dump({'source_commit':subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip(),'python':sys.version,'recursion_limit':sys.getrecursionlimit(),'command':'python generate-panel-depth.py <checkout-Python> <fixture>','cases':rows},ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} casos profundos do painel capturados')
