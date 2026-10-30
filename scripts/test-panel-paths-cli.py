#!/usr/bin/env python3
"""Caminhos do painel pela interface pública com disco e rede temporários."""
import json,os,subprocess,sys,tempfile,threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
binary=Path(sys.argv[1]).resolve();fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-paths.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  body=json.dumps([{'name':'fixture','state':'idle','cwd':self.server.cwd}] if self.path=='/api/sessions' else []).encode();self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();failures=[]
try:
 with tempfile.TemporaryDirectory() as tmp:
  root=Path(tmp);(root/'real/nested').mkdir(parents=True);(root/'link').symlink_to('real/nested');(root/'dangling').symlink_to('absent/nested');(root/'cycle-a').symlink_to('cycle-b');(root/'cycle-b').symlink_to('cycle-a')
  config=root/'.hangar/config';config.mkdir(parents=True);(config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\n');(config/'peers.json').write_text('{}')
  for row in fixture['cases']:
   server.cwd=row['cwd'];result=subprocess.run([str(binary),'panel-data'],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp},cwd=tmp,capture_output=True,timeout=10)
   assert result.returncode==0,(row,result.stderr)
   actual=json.loads(result.stdout)['sessions'][0]['cwd_real'].replace(tmp,'<ROOT>')
   if actual!=row['result']:failures.append((row,actual))
 assert not failures,failures
 print('8 caminhos do snapshot conferidos, incluindo ancestral simbólico e destino ausente')
finally:server.shutdown();server.server_close();thread.join()
