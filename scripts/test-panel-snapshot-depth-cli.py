#!/usr/bin/env python3
"""Confere o snapshot profundo pelo CLI sem servidor ou configuração reais."""
import json,os,subprocess,sys,tempfile,threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
binary=Path(sys.argv[1]).resolve()
fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-snapshot-depth.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  body=self.server.row['sessions_json' if self.path=='/api/sessions' else 'projects_json'].encode()
  self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();failures=[];count=0
try:
 for row in fixture['cases']:
  expected=row['results']['original']
  if 'exception' in expected:expected=row['results']['encoder-reference']
  with tempfile.TemporaryDirectory() as tmp:
   config=Path(tmp)/'.hangar/config';config.mkdir(parents=True)
   (config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\n')
   (config/'peers.json').write_text(row['peers_json']);server.row=row
   result=subprocess.run([str(binary),'panel-data'],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp},cwd=tmp,capture_output=True,timeout=15)
   label=f'{row["name"]}/{row["depth"]}'
   if result.returncode!=expected['rc']:failures.append((label,'rc',result.returncode));continue
   if result.stdout!=expected['stdout'].encode():failures.append((label,'stdout',len(result.stdout),len(expected['stdout'].encode())))
   count+=1
 assert not failures,failures
 print(f'{count} snapshots conferidos byte a byte: 10 originais e 2 referências D44')
finally:
 server.shutdown();server.server_close();thread.join()
