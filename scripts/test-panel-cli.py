#!/usr/bin/env python3
"""Confere contratos do painel pelo executável nativo, com ambiente temporário."""
import json, os, subprocess, sys, tempfile
from pathlib import Path
binary=Path(sys.argv[1]).resolve()
fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel.json').read_text())
with tempfile.TemporaryDirectory() as tmp:
 environment={**os.environ,'HOME':tmp,'USERPROFILE':tmp}
 for row in fixture['cases']:
  if row['kind']!='dispatch':continue
  result=subprocess.run([str(binary),'panel-action',*row['args']],env=environment,cwd=tmp,capture_output=True,text=True,timeout=10)
  assert result.returncode==row['rc'],(row['args'],result.returncode,row['rc'])
  actual=json.loads(result.stdout) if result.stdout.strip() else None
  assert actual==row['result'],(row['args'],actual,row['result'])
print('6 contratos públicos de despacho do painel conferidos')

# Erros de transporte devem atravessar o comando público com rc1 e JSON legível.
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
payloads=[(200,b"\xff",None),(503,b"[]","503: Synthetic reason"),(503,b"null","503: Synthetic reason"),(503,b'"text"',"503: Synthetic reason"),(503,b"1","503: Synthetic reason"),(503,b'{"detail":null}',"503: Synthetic reason"),(503,b'{"detail":{"reason":true}}',"503: {'reason': True}")]
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  self.rfile.read(int(self.headers.get('Content-Length','0')))
  status,body,_=payloads[self.server.case]
  self.send_response(status,'Synthetic reason');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
try:
 with tempfile.TemporaryDirectory() as tmp:
  config=Path(tmp)/'.hangar/config';config.mkdir(parents=True)
  (config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\n')
  environment={**os.environ,'HOME':tmp,'USERPROFILE':tmp}
  for index,(_,_,expected) in enumerate(payloads):
   server.case=index
   result=subprocess.run([str(binary),'panel-action','fixture','rename','renamed'],env=environment,cwd=tmp,capture_output=True,text=True,timeout=10)
   assert result.returncode==1,(index,result.returncode,result.stdout,result.stderr)
   actual=json.loads(result.stdout)
   assert actual['ok'] is False,(index,actual)
   if expected is None:assert actual['message'].startswith(f'resposta não-JSON de http://127.0.0.1:{server.server_port}: '),actual
   else:assert actual['message']==expected,(index,actual,expected)
 print('7 erros públicos do painel conferidos com rc1 e JSON')
finally:
 server.shutdown();server.server_close();thread.join()
