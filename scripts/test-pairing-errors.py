"""Configuração inválida recusa antes de imprimir credenciais ou URLs."""
import os,pathlib,subprocess,tempfile
root=pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as temporary:
 home=pathlib.Path(temporary);backend=home/'backend';backend.mkdir();file=backend/'.env'
 base={'HOME':str(home),'USERPROFILE':str(home),'PATH':str(home),'CP_AUTH_TOKEN':'fixture-sensitive','CP_LAN_BIND_IP':'192.0.2.10'}
 errors=[]
 for command in [['pairing'],['doctor','--qr'],['doctor']]:
  for name,env,content,expected in [
   ('port',{**base,'CP_PORT':'bad'},b'',b'pairing_config_invalid: CP_PORT\n'),
   ('front',{**base,'CP_FRONT_PORT':'bad'},b'',b'pairing_config_invalid: CP_FRONT_PORT\n'),
   ('file',base,b'CP_PUBLIC_URL=fixture-sensitive\xff\n',None),
   ('environment',{**base,'CP_AUTH_TOKEN':b'fixture-sensitive\xff'.decode('utf-8','surrogateescape')},b'',b'pairing_config_invalid: CP_AUTH_TOKEN\n')]:
   file.write_bytes(content)
   result=subprocess.run([str(root/'crates/target/debug/hangar-server'),*command,'--backend',str(backend)],cwd=home,env=env,capture_output=True,timeout=10)
   good=(result.returncode==1 and not result.stdout and b'fixture-sensitive' not in result.stderr and (result.stderr==expected if expected else result.stderr.startswith(b'pairing_config_unavailable: ')))
   if not good:errors.append((command,name,result.returncode,result.stdout,result.stderr))
 assert not errors,errors
 print('GREEN:12 recusas exatas, sem stdout nem valor sensível no diagnóstico.')
