"""O diagnóstico recebe a instalação explicitamente, sem procurar outra pelo cwd."""
import json,os,pathlib,subprocess,tempfile
root=pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as temporary:
 home=pathlib.Path(temporary);backend=home/'installation/backend';backend.mkdir(parents=True)
 (backend/'.env').write_text('CP_PORT=9876\nCP_AUTH_TOKEN=fixture-token\nCP_LAN_BIND_IP=192.0.2.10\n')
 environment={'HOME':str(home),'USERPROFILE':str(home),'PATH':os.environ['PATH']}
 result=subprocess.run([str(root/'crates/target/debug/hangar-server'),'doctor','--backend',str(backend),'--qr'],cwd=home,env=environment,capture_output=True,timeout=10)
 errors=[]
 if (result.returncode,result.stdout)!=(2,b'  Abra no celular: http://192.0.2.10:9876/?token=fixture-token\n\n'):errors.append(('backend',result.returncode,result.stdout,result.stderr))
 for case in json.loads((root/'backend/tests/fixtures/contract/peers/pairing-print.json').read_text())['cases']:
  if case['tty']:continue
  values=case['settings']
  if values['lan_bind_ip'] in {'127.0.0.1','localhost','::1','0.0.0.0','auto'}:values={**values,'lan_bind_ip':'192.0.2.10'}
  env={**environment,**{'CP_'+key.upper():str(value) for key,value in values.items()}}
  result=subprocess.run([str(root/'crates/target/debug/hangar-server'),'doctor','--qr'],cwd=home,env=env,capture_output=True,timeout=10)
  if (result.returncode,result.stdout)!=(case['doctor_rc'],case['doctor_stdout'].encode()):errors.append((case['name'],result.returncode,result.stdout,result.stderr))
 assert not errors,errors
 print('GREEN: doctor leu apenas a instalação explícita, sem servidor.')
