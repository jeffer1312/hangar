"""Prova Linux isolada do instalador nativo de binários, sem serviço ou Python de produção."""
import hashlib,http.server,json,os,pathlib,subprocess,tempfile,threading
binary=pathlib.Path('crates/target/debug/hangar-server').resolve()
content={n:('new-'+n).encode() for n in ['hangar-auth','hangar-cano','hangar-server']}
manifest={'commit':'b'*40,'files':{'linux-x86_64/'+n:{'name':n,'sha256':hashlib.sha256(b).hexdigest()} for n,b in content.items()}}
urls=[]
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  urls.append(self.path);name=self.path.rsplit('/',1)[-1];data=json.dumps(manifest).encode() if name=='server-latest.json' else content.get(name)
  self.send_response(200 if data else 404);self.end_headers();self.wfile.write(data or b'')
with tempfile.TemporaryDirectory() as tmp:
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
 try:
  env={'HOME':tmp,'USERPROFILE':tmp,'PATH':os.environ['PATH'],'HANGAR_SERVER_RELEASE_URL':f'http://127.0.0.1:{server.server_port}/release','HANGAR_UPDATE_OWNS_RUST':'1'}
  result=subprocess.run([str(binary),'rust-release',tmp],env=env,capture_output=True,text=True,timeout=30)
  assert result.returncode==0,(result.returncode,result.stdout,result.stderr)
  dest=pathlib.Path(tmp)/'.hangar/bin';actual={str(p.relative_to(dest)):p.read_text() for p in dest.rglob('*') if p.is_file()}
  fixture=json.load(open('backend/tests/fixtures/contract/update-git/auth-release.json'))['cases'][0]['after_publish'];assert actual==fixture,(actual,fixture)
  assert urls==['/release/server-latest.json','/release/hangar-auth','/release/hangar-cano','/release/hangar-server'],urls
  print('GREEN: CLI rust-release publicou o par/autenticação e cano sem Python, com HANGAR_UPDATE_OWNS_RUST=1 herdado; 4 pedidos locais e bytes iguais à captura Python.')
 finally:server.shutdown();server.server_close();thread.join()
