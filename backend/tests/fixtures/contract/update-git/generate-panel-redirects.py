"""Captura métodos e cabeçalhos em redirects e erros HTTP do transporte do painel."""
import json,os,runpy,subprocess,sys,tempfile,threading,time
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();calls=[];rows=[]
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):self.respond()
 def do_POST(self):self.respond()
 def respond(self):
  body=self.rfile.read(int(self.headers.get('Content-Length','0'))).decode()
  calls.append(dict(path=self.path,method=self.command,authorization=self.headers.get('Authorization'),content_type=self.headers.get('Content-Type'),body=body))
  if self.path=='/loop' or self.path.startswith('/chain/') or self.path=='/slow-chain':
   if self.path=='/slow-chain':time.sleep(.6)
   location=self.path if self.path=='/loop' else '/slow-end' if self.path=='/slow-chain' else '/chain/'+str(int(self.path.rsplit('/',1)[1])+1)
   self.send_response(302,'Synthetic reason');self.send_header('Location',location);self.send_header('Content-Length','0');self.end_headers();return
  if self.path=='/slow-end':time.sleep(.6)
  if self.path.startswith('/r/'):
   self.send_response(int(self.path[3:]),'Synthetic reason');self.send_header('Location','/ok');self.send_header('Content-Length','0');self.end_headers();return
  payload=b'{"ok":true}';self.send_response(200);self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with tempfile.TemporaryDirectory() as tmp:
  os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp);runpy.run_path(str(reference/'scripts/hangar-panel-action'));common=sys.modules['hangar_panel_common']
  specs=[(f'/r/{status}',method,3) for status in [300,301,302,303,304,305,307,308] for method in ['GET','POST']]+[('/loop','GET',3),('/chain/0','GET',3),('/slow-chain','GET',1)]
  for path,method,timeout in specs:
    calls.clear();body={'message':'ação'} if method=='POST' else None
    with patch.object(common,'resolve',return_value=(f'http://127.0.0.1:{server.server_port}','fixture-token','session')):
     try:result={'value':common.api('session',method,path,body,timeout=timeout)}
     except common.PanelError as error:result={'error':str(error)}
    rows.append(dict(path=path,method=method,body=body,timeout=timeout,calls=list(calls),result=result))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-panel-redirects.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n');print(f'{len(rows)} casos de redirecionamento capturados')
finally:server.shutdown();server.server_close()
