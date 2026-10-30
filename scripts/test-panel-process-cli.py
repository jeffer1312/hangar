#!/usr/bin/env python3
"""Erros reais de lançamento e prazo do painel, com executáveis temporários."""
import json,os,subprocess,sys,tempfile,threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
binary=Path(sys.argv[1]).resolve()
fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-process-errors.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  body=b'[{"name":"fixture","tmux":"runner"}]';self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();failures=[]
try:
 for row in fixture['cases']:
  with tempfile.TemporaryDirectory() as tmp:
   config=Path(tmp)/'.hangar/config';config.mkdir(parents=True)
   (config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\n')
   name=row['name'];tmux=Path(tmp)/'tmux';term=Path(tmp)/'kitty'
   tmux.write_text('invalid executable' if name=='run-spawn' else f'#!{sys.executable}\nimport time\ntime.sleep(6)\n' if name=='run-timeout' else '#!/bin/sh\nexit 0\n');tmux.chmod(0o700)
   term.write_text('invalid executable' if name=='terminal-spawn' else '#!/bin/sh\nexit 0\n');term.chmod(0o700)
   result=subprocess.run([str(binary),'panel-action','fixture','project','open'],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp},cwd=tmp,capture_output=True,timeout=10)
   if result.returncode!=row['rc'] or result.stdout!=row['stdout'].encode():failures.append((name,result.returncode,result.stdout.decode()))
 assert not failures,failures
 print('3 erros reais de subprocesso do painel conferidos byte a byte')
finally:
 server.shutdown();server.server_close();thread.join()
