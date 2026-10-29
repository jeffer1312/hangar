"""Registra o comportamento Python anterior à recusa UTF-8 aprovada na D22."""
import argparse,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.clear();os.environ.update(HOME=tmp,USERPROFILE=tmp);sys.path.insert(0,str(ref/'backend'))
 from app.config import Settings
 rows=[]
 for key in ['CP_SERVER_ID','cp_server_id','Cp_Server_Id']:
  os.environb[key.encode()]=b'\xff';value=Settings(_env_file=Path(tmp)/'.env').server_id
  rows.append(dict(key=key,bytes=[255],python_value_escaped=value.encode('unicode_escape').decode()))
  del os.environb[key.encode()]
 out.write_text(json.dumps(dict(source_commit=commit,command='python backend/tests/fixtures/contract/update-git/generate-invalid-env.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/invalid-env.json',cases=rows),indent=2)+'\n')
