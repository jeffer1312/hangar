"""Captura escrita do painel em livro temporário e saída UTF-8 real."""
import contextlib,hashlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
 action=runpy.run_path(str(reference/'scripts/hangar-panel-action'));config=Path(tmp)/'.hangar/config';config.mkdir(parents=True)
 nested='['*1500+'0'+']'*1500
 cases=[('plain','{"peer":{"enabled":false,"extra":"é"}}'),('nan','{"peer":{"extra":NaN}}'),('infinity','{"peer":{"extra":Infinity}}'),('surrogate','{"peer":{"extra":"\\ud800"}}'),('literal','{"peer":{"extra":"'+chr(0x10ffff)+'"}}'),('deep-extra','{"peer":{"extra":'+nested+'}}'),('deep-replaced','{"peer":{"enabled":'+nested+'}}'),('syntax','{'),('root-array','[]'),('unknown','{}')]
 for name,raw in cases:
  path=config/'peers.json';path.write_text(raw);path.chmod(0o640)
  for old in config.glob('peers.json.*.tmp'):old.unlink()
  (config/'peers.json.stale.tmp').write_text('synthetic')
  sink=io.BytesIO();stdout=io.TextIOWrapper(sink,encoding='utf-8',errors='strict');result={}
  try:
   with patch.object(sys,'argv',['hangar-panel-action','peer','peer','on']),contextlib.redirect_stdout(stdout):rc=action['main']()
   result['rc']=rc
  except Exception as error:result.update(rc=1,exception=type(error).__name__,message=str(error).splitlines()[0])
  stdout.flush();result['stdout_hex']=sink.getvalue().hex();after=path.read_bytes();result.update(file_hex=after.hex(),mode=path.stat().st_mode&0o777,temps=sorted([{'size':p.stat().st_size,'mode':p.stat().st_mode&0o777} for p in config.glob('peers.json.*.tmp')],key=lambda x:(x['size'],x['mode'])))
  if name=='deep-extra':result['file_sha256']=hashlib.sha256(after).hexdigest();result['file_bytes']=len(after);del result['file_hex']
  rows.append(dict(name=name,input=raw,result=result))
output.write_text(json.dumps(dict(source_commit=commit,command='python generate-panel-write.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n')
print([(row['name'],row['result']['rc'],row['result'].get('exception'),row['result']['temps']) for row in rows])
