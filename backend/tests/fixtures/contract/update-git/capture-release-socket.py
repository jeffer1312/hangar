"""Prazo real do download original, com resposta local parada depois dos cabeçalhos."""
import io,json,os,sys,tempfile,subprocess,threading,time,http.server,types
from pathlib import Path
ref=Path(sys.argv[1]).resolve();out=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='release-socket-reference-') as td:
 os.environ.update(HOME=td,USERPROFILE=td);os.chdir(td);sys.path.insert(0,str(ref/'backend'))
 app=types.ModuleType('original_release');app.__file__=str(Path(td)/'app/rust_release.py');exec(compile(subprocess.check_output(['git','-C',str(ref),'show',f'{commit}:backend/app/rust_release.py']),app.__file__,'exec'),app.__dict__)
 class Handler(http.server.BaseHTTPRequestHandler):
  def log_message(self,*args):pass
  def do_GET(self):
   self.send_response(200);self.send_header('Content-Length','2');self.end_headers();self.wfile.flush();time.sleep(18)
   try:self.wfile.write(b'ok')
   except OSError:pass
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();started=time.monotonic()
 try:body=app._get(f'http://127.0.0.1:{server.server_port}/fixture',30);error=None
 except Exception as exc:body=None;error=type(exc).__name__
 elapsed=time.monotonic()-started;server.shutdown();server.server_close()
 out.write_text(json.dumps(dict(source_commit=commit,command='python capture-release-socket.py <reference> <output>',deadline=30,socket_timeout=app._SOCKET_TIMEOUT,response_delay=18,error=error,body=body.decode() if body is not None else None,elapsed_seconds=elapsed),indent=2)+'\n');print(error,round(elapsed,2))
