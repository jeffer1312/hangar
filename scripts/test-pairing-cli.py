"""Confere o pareamento sem serviço, com configuração e diretórios temporários."""
import json, os, pathlib, subprocess, tempfile
root=pathlib.Path(__file__).resolve().parents[1]
binary=root/'crates/target/debug/hangar-server'
fixture=json.loads((root/'backend/tests/fixtures/contract/peers/pairing-print.json').read_text())
with tempfile.TemporaryDirectory() as temporary:
 home=pathlib.Path(temporary);backend=home/'backend';backend.mkdir()
 checked=0
 for case in fixture['cases']:
  if case['tty']:continue
  values=case['settings']
  # A detecção de LAN tem sua prova própria; este CLI recebe o endereço capturado.
  if values['lan_bind_ip'] in {'127.0.0.1','localhost','::1','0.0.0.0','auto'}:values={**values,'lan_bind_ip':'192.0.2.10'}
  env={'HOME':str(home),'USERPROFILE':str(home),'PATH':os.environ['PATH'],**{'CP_'+key.upper():str(value) for key,value in values.items()}}
  result=subprocess.run([str(binary),'pairing','--backend',str(backend)],env=env,capture_output=True,timeout=10)
  assert (result.returncode,result.stdout)==(case['rc'],case['stdout'].encode()),(case['name'],result.returncode,result.stdout,result.stderr)
  doctor=subprocess.run([str(binary),'doctor','--backend',str(backend),'--qr'],env=env,capture_output=True,timeout=10)
  assert (doctor.returncode,doctor.stdout)==(case['doctor_rc'],case['doctor_stdout'].encode()),(case['name'],doctor.returncode,doctor.stdout,doctor.stderr)
  checked+=1
 print(f'GREEN: {checked} saídas de pareamento iguais ao Python, sem servidor.')
