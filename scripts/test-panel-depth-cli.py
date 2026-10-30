#!/usr/bin/env python3
"""Prova cópia, descarte e saída profunda do painel em subprocessos isolados."""
import json,os,subprocess,sys,tempfile,threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
binary=Path(sys.argv[1]).resolve()
fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-depth.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):self.reply()
 def do_POST(self):self.reply()
 def do_DELETE(self):self.reply()
 def reply(self):
  self.rfile.read(int(self.headers.get('Content-Length','0')))
  response=self.server.responses.pop(0);status,body=response if isinstance(response,tuple) else (200,response);body=body.encode();self.send_response(status);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();failures=[]
try:
 for row in fixture['cases']:
  with tempfile.TemporaryDirectory() as tmp:
   config=Path(tmp)/'.hangar/config';config.mkdir(parents=True)
   (config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\n')
   nested='['*row['depth']+'0'+']'*row['depth'];name=row['name']
   args={'pane':['fixture','project','pane'],'kill':['fixture','kill'],'start':['fixture','project','start'],'info':['fixture','info'],'info-error':['fixture','info'],'commit':['fixture','commit'],'import':['fixture','import-candidates']}[name]
   server.responses={'pane':['{"pane":'+nested+'}'],'kill':['{"ignored":'+nested+'}'],'start':['{"state":"working","extra":'+nested+'}'],'info':['[{"name":"fixture","branch":'+nested+'}]','[]'],'info-error':['[{"name":"fixture","branch":'+nested+'}]',(400,'{"detail":"not a git repository"}')],'commit':['[{"path":'+nested+'}]','{}'],'import':['[{"name":'+nested+',"cwd":"/remote/missing","command":"","port":'+nested+'}]']}[name]
   result=subprocess.run([str(binary),'panel-action',*args],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp},cwd=tmp,capture_output=True,timeout=15)
   expected=row['results']['default'];label=f'{name}/{row["depth"]}'
   if result.returncode!=expected['rc']:failures.append((label,'rc',result.returncode));continue
   if result.stdout!=expected['stdout'].encode():failures.append((label,'stdout',len(result.stdout),expected['stdout_bytes']))
 assert not failures,failures
 print(f'{len(fixture["cases"])} contratos profundos do painel conferidos byte a byte')
finally:
 server.shutdown();server.server_close();thread.join()
