"""Recusa portas inválidas antes das sondas; uma interposição bloqueia qualquer conexão no RED."""
import json,os,pathlib,subprocess,tempfile
root=pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as temporary:
 home=pathlib.Path(temporary);backend=home/'backend';backend.mkdir()
 source=home/'deny.c';source.write_text('#include <errno.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <sys/socket.h>\nint connect(int fd,const struct sockaddr *a,socklen_t n){const char *p=getenv("CONNECT_LOG");FILE *f=fopen(p,"a");if(f){fputs("blocked\\n",f);fclose(f);}errno=ECONNREFUSED;return -1;}\n')
 library=home/'deny.so';subprocess.run(['cc','-shared','-fPIC',str(source),'-o',str(library)],check=True)
 failures=[]
 for port in [-1,65536,10**30]:
  log=home/'connections';log.unlink(missing_ok=True)
  env={'HOME':str(home),'USERPROFILE':str(home),'PATH':str(home),'CP_PORT':str(port),'CP_AUTH_TOKEN':'fixture-token','CP_LAN_BIND_IP':'127.0.0.1','LD_PRELOAD':str(library),'CONNECT_LOG':str(log)}
  result=subprocess.run([str(root/'crates/target/debug/hangar-server'),'doctor','--backend',str(backend)],cwd=home,env=env,capture_output=True,timeout=15)
  expected=(1,b'',b'doctor_config_invalid: CP_PORT fora de 0..65535\n')
  if (result.returncode,result.stdout,result.stderr)!=expected or log.exists():failures.append({'port':port,'code':result.returncode,'stdout':result.stdout.decode(errors='replace'),'stderr':result.stderr.decode(errors='replace'),'blocked_connections':log.read_text().count('blocked') if log.exists() else 0})
 assert not failures,json.dumps(failures,ensure_ascii=False)
 print('GREEN: três portas recusadas antes de qualquer conexão, stdout vazio e diagnóstico exato.')
