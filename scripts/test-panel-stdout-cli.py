#!/usr/bin/env python3
"""Confere stdout e diagnóstico Unicode do painel em HTTP temporário."""
import json,os,subprocess,sys,tempfile,threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
binary=Path(sys.argv[1]).resolve();fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-stdout.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  row=self.server.row;value=row['value_json']
  body=('[]' if self.path=='/api/sessions' else '[{"name":"fixture","extra":'+value+'}]' if self.path=='/api/projects' else '{"pane":'+value+'}').encode()
  self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();failures=[]
try:
 for row in fixture['cases']:
  with tempfile.TemporaryDirectory() as tmp:
   config=Path(tmp)/'.hangar/config';config.mkdir(parents=True);(config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\n');(config/'peers.json').write_text('{}');server.row=row
   args=['panel-action','fixture','project','pane'] if row['command']=='action' else ['panel-data']
   result=subprocess.run([str(binary),*args],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp},cwd=tmp,capture_output=True,timeout=10)
   invalid='exception' in row['result'];expected=b'' if invalid else bytes.fromhex(row['result']['stdout_hex'])
   if result.returncode!=row['result']['rc'] or result.stdout!=expected or (invalid and result.stderr!='erro: saída do painel contém Unicode inválido\n'.encode()):failures.append((row['name'],row['command'],result.returncode,result.stdout.hex(),result.stderr.decode(errors='replace')))
 assert not failures,failures
 print('8 contratos de stdout do painel conferidos, com recusa Unicode explícita antes da publicação')
finally:server.shutdown();server.server_close();thread.join()
