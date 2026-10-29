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
calls=[]
scenario=""
raw="Hoje vamos conferir o ditado completo."
class Service(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_POST(self):
        data=self.rfile.read(int(self.headers.get('content-length',0)))
        from urllib.parse import urlparse, parse_qs
        parsed=urlparse(self.path)
        calls.append({'path':parsed.path,'query':{k:v[-1] for k,v in parse_qs(parsed.query).items()},'payload':json.loads(data) if '/dictation/' in self.path else None})
        if '/status' in self.path:
            result={'providers':[]}
        elif '/dictation/catalog' in self.path:
            result={'provider':'claude','models':[],'generation':'fixture'}
        elif '/dictation/' in self.path:
            payload=json.loads(data)
            result={'text':payload['raw'],'raw':payload['raw'],'organization_mode':'none','estilo_aplicado':'cru','aviso':None,'organization_code':None}
        else:
            result={'text':raw,'provider':'fixture'}
            if scenario=='warnings': result['aviso']='Transcrito pelo fixture: primeiro falhou (500)'
        if scenario=='warnings' and '/dictation/' in self.path:
            result.update(organization_mode='external_api',aviso='Configure a chave da API de organização em Configurações → Voz; foi mantida a transcrição original.',organization_code='dictation_api_key_missing')
        envelope={'ok':True,'result':result}
        if scenario=='stt_error': envelope={'ok':False,'error':{'status':503,'code':'transcription_not_configured','detail':'Nenhum serviço de transcrição está configurado.'}}
        body=json.dumps(envelope).encode()
        self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Service)
thread=threading.Thread(target=server.serve_forever);thread.start()
settings.auth_token='fixture-owner'
transcription_bridge.configure(f'127.0.0.1:{server.server_port}','fixture-private')
cases=[]
# Sem lifespan: só a borda ASGI original, sem relançar sessões nem hooks.
import socket
import urllib.request
import urllib.error
import uvicorn
socket_=socket.socket();socket_.bind(('127.0.0.1',0));socket_.listen()
http_server=uvicorn.Server(uvicorn.Config(api.app,lifespan='off',log_level='critical',access_log=False))
http_thread=threading.Thread(target=lambda:http_server.run(sockets=[socket_]));http_thread.start()
try:
    client=TestClient(api.app,raise_server_exceptions=False)
    variants=[('default','',{},b'audio'),('header','?limpar=0&name=query.wav',{'x-filename':'header.webm'},b'audio'),('query','?limpar=0&name=query.wav',{},b'audio'),('empty_header','?limpar=0&name=query.wav',{'x-filename':''},b'audio'),('warnings','?organization_mode=external_api',{},b'audio'),('stt_error','?limpar=0',{},b'audio'),('parameters','?organization_mode=none&organization_model=chosen&include_recent_messages=1&session=ignored&generation=ignored&organization_account=ignored',{},b'audio'),('declared_limit','',{'content-length':str(100*1024*1024+1)},b'audio'),('real_limit','',{},None)]
    for scenario,query,headers,content in variants:
        calls.clear()
        if scenario=='real_limit':
            request=urllib.request.Request(f'http://127.0.0.1:{socket_.getsockname()[1]}/api/dictation/transcribe',data=(b'x'*(1024*1024) for _ in range(101)),headers={'Authorization':'Bearer fixture-owner','Content-Type':'application/octet-stream'},method='POST')
            try: response=urllib.request.urlopen(request,timeout=15)
            except urllib.error.HTTPError as error: response=error
            cases.append({'id':scenario,'path':'/api/dictation/transcribe','headers':{},'transport':'chunked','status':response.status,'response':response.read().decode(),'calls':list(calls)})
            continue
        response=client.post('/api/dictation/transcribe'+query,content=content,headers={'authorization':'Bearer fixture-owner',**headers})
        cases.append({'id':scenario,'path':'/api/dictation/transcribe'+query,'headers':headers,'status':response.status_code,'response':response.json() if response.headers.get('content-type','').startswith('application/json') else response.text,'calls':list(calls)})
    for path in ['/api/ditado/relimpar','/api/transcription/providers/fixture/test']:
        calls.clear()
        request=urllib.request.Request(f'http://127.0.0.1:{socket_.getsockname()[1]}{path}',data=(b'x'*(1024*1024) for _ in range(101)),headers={'Authorization':'Bearer fixture-owner','Content-Type':'application/json'},method='POST')
        try: response=urllib.request.urlopen(request,timeout=15)
        except urllib.error.HTTPError as error: response=error
        content=response.read().decode()
        cases.append({'id':'reorganize_real_limit' if 'relimpar' in path else 'provider_real_limit','path':path,'headers':{},'transport':'chunked','status':response.status,'response':json.loads(content) if response.headers.get('content-type','').startswith('application/json') else content,'calls':list(calls)})
    from unittest.mock import patch
    for path in ['/api/dictation/transcribe','/api/transcription/providers/fixture/test']:
        with patch.object(transcription_bridge._opener,'open',side_effect=TimeoutError) as network:
            response=client.post(path,content=b'audio',headers={'Authorization':'Bearer fixture-owner'})
        assert network.call_args.kwargs['timeout']==180
        cases.append({'id':'public_timeout' if '/dictation/' in path else 'provider_timeout','path':path,'headers':{},'status':response.status_code,'response':response.json(),'timeout_seconds':180})
    for path,seconds in [('/api/dictation/models?session=destination&fresh=true',60),('/api/transcription/providers/status',15)]:
        with patch.object(transcription_bridge._opener,'open',side_effect=TimeoutError) as network:
            response=client.get(path,headers={'Authorization':'Bearer fixture-owner'})
        assert network.call_args.kwargs['timeout']==seconds
        cases.append({'id':'catalog_timeout' if seconds==60 else 'status_timeout','path':path,'headers':{},'status':response.status_code,'response':response.json(),'timeout_seconds':seconds})
    assert not list(Path(scratch.name).rglob('*.wav'))

finally:
    http_server.should_exit=True;http_thread.join(timeout=5);socket_.close()
    server.shutdown();server.server_close();thread.join()
output={'source_commit':source_commit,'command':'cd backend && uv run python tests/fixtures/contract/dictation_public/capture_edges.py <backend-fonte>','cases':cases}
Path(__file__).with_name('edges.json').write_text(json.dumps(output,ensure_ascii=False,indent=2)+'\n')

scratch.cleanup()
