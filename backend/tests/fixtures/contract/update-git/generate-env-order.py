"""Captura a ordem do ambiente e a interpolação com nomes de caixa diferente."""
import os,sys,tempfile,json,subprocess
from pathlib import Path
root=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();sys.path.insert(0,str(root/'backend'))
rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.clear();os.environ.update(HOME=tmp,USERPROFILE=tmp)
 from app.config import Settings
 path=Path(tmp)/'.env';path.write_text('CP_PUBLIC_URL=${CP_SERVER_ID}\n')
 for pairs in [[('CP_SERVER_ID','upper'),('cp_server_id','lower')],[('cp_server_id','lower'),('CP_SERVER_ID','upper')]]:
  os.environ.clear();os.environ.update(HOME=tmp,USERPROFILE=tmp);os.environ.update(pairs)
  settings=Settings(_env_file=path)
  rows.append(dict(environment=pairs,server_id=settings.server_id,public_url=settings.public_url))
output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'command':'python generate-env-order.py <checkout-Python> <fixture>','cases':rows},indent=2)+'\n')
print(json.dumps(rows))
