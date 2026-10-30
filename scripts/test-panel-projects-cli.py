#!/usr/bin/env python3
"""Prova projetos pelo CLI nativo com HTTP e programas substitutos isolados."""
import json, os, subprocess, sys, tempfile, threading, time
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
binary=Path(sys.argv[1]).resolve()
fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-projects.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  body=json.dumps(self.server.projects).encode();self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();count=0
try:
 for row in fixture['cases']:
  if row.get('failure'):continue
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp);config=root/'.hangar/config';config.mkdir(parents=True)
   (config/'.env').write_text(f'CP_PORT={server.server_port}\nCP_AUTH_TOKEN=fixture-token\nCP_SCAN_ROOTS={root}/first,{root}/second\n')
   (config/'peers.json').write_text('{}')
   for directory in ['first/unique','first/collision','second/collision']:(root/directory).mkdir(parents=True)
   log=root/'calls.jsonl'
   def program(name,stdout='',code=0):
    target=root/name
    target.write_text(f'#!{sys.executable}\nimport json,sys\nwith open({str(log)!r},"a") as f:f.write(json.dumps([{name!r},*sys.argv[1:]])+"\\n")\nsys.stdout.write({stdout!r})\nraise SystemExit({code})\n')
    target.chmod(0o700)
   kind=row['kind'];server.projects=row.get('projects',row.get('remote',[]))
   if kind=='open':
    if row['term']:program(row['term'])
    program('tmux',code=row['code']);args=['project','project','open'];expected=row['result']['value']
   elif kind=='pick':
    if row['term']:program(row['term'],row['stdout'],row['code'])
    args=['fixture','pick-folder','/start'];expected=row['result']
   else:args=['remote','import-candidates'];expected=row['result']
   environment={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp}
   result=subprocess.run([str(binary),'panel-action',*args],env=environment,cwd=tmp,capture_output=True,text=True,timeout=10)
   actual=json.loads(result.stdout.replace(tmp,'<ROOT>'))
   assert actual==expected,(row,actual,expected,result.stderr)
   assert result.returncode==(0 if expected['ok'] else 1),(row,result.returncode)
   expected_calls=[call['args'] for call in row.get('calls',[]) if call.get('kind')!='api']
   deadline=time.monotonic()+2
   while True:
    calls=[json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    if len(calls)>=len(expected_calls) or time.monotonic()>=deadline:break
    threading.Event().wait(.01)
   assert calls==expected_calls,(row,calls,expected_calls)
   count+=1
 print(f'{count} contratos de projetos conferidos pelo CLI; dois erros de subprocesso aguardam injeção específica')
finally:
 server.shutdown();server.server_close();thread.join()
