"""Captura transporte real do painel em HTTP de loopback, sem backend."""
import contextlib,io,json,os,runpy,subprocess,sys,tempfile,threading,time
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip();rows=[];calls=[]
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):self.respond()
 def do_POST(self):self.respond()
 def respond(self):
  body=self.rfile.read(int(self.headers.get('Content-Length','0'))).decode()
  calls.append(dict(path=self.path,method=self.command,authorization=self.headers.get('Authorization'),content_type=self.headers.get('Content-Type'),body=body))
  if self.path in ['/same','/cross']:
   self.send_response(302);self.send_header('Location',('/ok' if self.path=='/same' else f'http://127.0.0.1:{other.server_port}/ok'));self.end_headers();return
  if self.path=='/slow':time.sleep(1.3)
  status=503 if self.path=='/error-bytes' else 200
  payload=b'\xff'
  self.send_response(status,'Synthetic reason');self.send_header('Content-Length',str(len(payload)));self.end_headers()
  try:self.wfile.write(payload)
  except (BrokenPipeError,ConnectionResetError):pass
servers=[]
try:
 for _ in range(2):
  server=ThreadingHTTPServer(('127.0.0.1',0),Handler);servers.append(server);threading.Thread(target=server.serve_forever,daemon=True).start()
 first,other=servers
 with tempfile.TemporaryDirectory() as tmp:
  os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
  runpy.run_path(str(reference/'scripts/hangar-panel-action'));common=sys.modules['hangar_panel_common']
  for path,method,body,timeout in [('/error-bytes','GET',None,3),('/invalid-bytes','GET',None,3)]:
   calls.clear();base=f'http://127.0.0.1:{first.server_port}'
   with patch.object(common,'resolve',return_value=(base,'fixture-token','session')):
    try:result={'value':common.api('session',method,path,body,timeout=timeout)}
    except common.PanelError as error:result={'error':str(error)}
    except Exception as error:result={'exception':type(error).__name__,'message':str(error)}
   rows.append(dict(path=path,method=method,body=body,timeout=timeout,calls=list(calls),result=result))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-panel-encoding.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2).replace(f'http://127.0.0.1:{first.server_port}','<BASE>')+'\n')
 print(f'{len(rows)} casos de codificação HTTP do painel capturados')
finally:
 for server in servers:server.shutdown();server.server_close()
