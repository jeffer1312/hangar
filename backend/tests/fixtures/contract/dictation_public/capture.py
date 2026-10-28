"""Caracteriza a borda Python; execute antes de removê-la: uv run python tests/fixtures/contract/dictation_public/capture.py."""
import json
import subprocess
import sys
import threading
import os
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
source = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[4]
source_commit = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
if source_commit != 'ba2db1021ab7fb801eca50a50f9509dcb825ac7e':
    raise SystemExit('Use o checkout-fonte ba2db1021; a saída Rust nunca atualiza esta fixture.')
if subprocess.run(['git', '-C', str(source), 'diff', 'HEAD', '--quiet', '--', 'app/api.py', 'app/dictation_bridge.py', 'app/transcribe.py']).returncode:
    raise SystemExit('O checkout-fonte tem alterações nos contratos Python.')
sys.path.insert(0, str(source))
scratch = tempfile.TemporaryDirectory(prefix='dictation-contract-')
os.environ.update(HOME=scratch.name, USERPROFILE=scratch.name, XDG_CONFIG_HOME=scratch.name, GIT_CONFIG_GLOBAL=os.devnull)
from fastapi.testclient import TestClient
from app import api, transcription_bridge
from app.config import settings
class Service(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_POST(self):
        data=self.rfile.read(int(self.headers.get('content-length',0)))
        if '/status' in self.path:
            result={'providers':[]}
        elif '/dictation/catalog' in self.path:
            result={'provider':'claude','models':[],'generation':'fixture'}
        elif '/dictation/' in self.path:
            payload=json.loads(data)
            result={'text':payload['raw'],'raw':payload['raw'],'organization_mode':'none','estilo_aplicado':'cru','aviso':None,'organization_code':None}
        else:
            result={'text':'Fala original.','provider':'fixture'}
        body=json.dumps({'ok':True,'result':result}).encode()
        self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Service)
thread=threading.Thread(target=server.serve_forever);thread.start()
settings.auth_token='fixture-owner'
transcription_bridge.configure(f'127.0.0.1:{server.server_port}','fixture-private')
cases=[]
try:
    client=TestClient(api.app)
    for path,body in [('/api/ditado/relimpar',{'texto':'fala','estilo':'prosa','include_recent_messages':1.5}),('/api/dictation/transcribe?limpar=bad&include_recent_messages=bad',None),('/api/dictation/models?fresh=bad',None),('/api/dictation/transcribe?limpar=0',None),('/api/dictation/transcribe?limpar=bad',None),('/api/ditado/relimpar',{'texto':'Fala original.','estilo':'limpar','organization_mode':'none'}),('/api/ditado/relimpar',{'texto':'','estilo':'limpar'}),('/api/ditado/relimpar',{'texto':'Fala original.','estilo':'errado'}),('/api/ditado/relimpar',{'texto':'fala','estilo':'prosa','extra':1}),('/api/ditado/relimpar',{}),('/api/dictation/models',None),('/api/dictation/models?session=x&fresh=bad',None),('/api/transcription/providers/status',None),('/api/ditado/relimpar',{'texto':12,'estilo':False}),('/api/ditado/relimpar',{'texto':'fala','estilo':'prosa','recent_messages':[3]}),('/api/ditado/relimpar',{'texto':'fala','estilo':'prosa','include_recent_messages':'sim'}),('/api/ditado/relimpar',{'texto':'fala','estilo':'prosa','include_recent_messages':'1'})]:
        method='GET' if '/models' in path or path.endswith('/status') else 'POST'
        r=client.request(method,path,headers={'Authorization':'Bearer fixture-owner'},**({'json':body} if body is not None else {'content':b'audio'}))
        cases.append({'method':method,'path':path,'body':body,'status':r.status_code,'response':r.json()})
    for content_type in ['text/plain','application/json; charset=utf-8','application/vnd.hangar+json']:
        raw_body='{"texto":"fala","estilo":"prosa","organization_mode":"none"}'
        response=client.post('/api/ditado/relimpar',content=raw_body,headers={'Authorization':'Bearer fixture-owner','Content-Type':content_type})
        cases.append({'method':'POST','path':'/api/ditado/relimpar','body':None,'raw_body':raw_body,'headers':{'content-type':content_type},'status':response.status_code,'response':response.json()})

finally:
    server.shutdown();server.server_close();thread.join()
output={'source_commit':source_commit,'command':'cd backend && uv run python tests/fixtures/contract/dictation_public/capture.py','cases':cases}
Path(__file__).with_name('responses.json').write_text(json.dumps(output,ensure_ascii=False,indent=2)+'\n')

scratch.cleanup()
