"""Captura contratos executando o Python de referência, em diretórios isolados."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--reference', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
reference = args.reference.resolve()
output = args.output.resolve()
commit = subprocess.check_output(['git', '-C', str(reference), 'rev-parse', 'HEAD'], text=True).strip()
if not (reference / 'backend/app/peers_api.py').is_file():
    raise SystemExit('Python de referência ausente: a caracterização não rodou.')
sys.path.insert(0, str(reference / 'backend'))
with tempfile.TemporaryDirectory(prefix='hangar-peers-reference-') as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CP_AUTH_TOKEN='contract-owner', CP_SERVER_ID='fixture', CP_LAN_BIND_IP='127.0.0.1', CP_PORT='8765')
    os.chdir(root)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import alcance, peers, peers_api, peers_check
    from app.config import settings
    app = FastAPI()
    app.include_router(peers_api.peers_router)
    app.include_router(alcance.alcance_router)
    client = TestClient(app)
    peers._PEERS_FILE = root / 'peers.json'
    peers_api._ENV_ARQUIVO = root / '.env'
    settings.auth_token = 'contract-owner'
    settings.server_id = 'fixture'
    settings.lan_bind_ip = '127.0.0.1'
    settings.port = 8765
    settings.front_port = 5173
    settings.public_url = ''
    alcance._detectar_lan = lambda: '192.0.2.10'
    peers_api.detect_lan_ip = lambda: '192.0.2.10'
    alcance._nome_tailscale = lambda: ''
    alcance._status_tailscale = lambda: {}
    alcance._bater = lambda url: 12.5
    peers_check.time.monotonic = lambda: 100.0
    peers_check._bater = lambda *a, **kw: (200, {'identificador': 'remote'})
    cases = []
    def capture(name, method, path, body=None, initial=None, raw=None, auth=True, initial_env='CP_FRONT_PORT=5173\n', wire_body=None, difference=None):
        if peers._PEERS_FILE.exists(): peers._PEERS_FILE.unlink()
        if raw is not None: peers._PEERS_FILE.write_text(raw)
        elif initial is not None: peers._PEERS_FILE.write_text(json.dumps(initial, ensure_ascii=False))
        peers_api._ENV_ARQUIVO.write_text(initial_env)
        settings.server_id = 'fixture'
        req = {'headers': {'Authorization':'Bearer contract-owner'}} if auth else {}
        if body is not None: req['json'] = body
        if wire_body is not None:
            req['content']=wire_body
            req.setdefault('headers',{})['Content-Type']='application/json'
        response = client.request(method, path, **req)
        effect = peers._PEERS_FILE.read_text() if peers._PEERS_FILE.exists() else None
        try: effect = json.loads(effect) if effect is not None else None
        except ValueError: pass
        cases.append(dict(name=name,method=method,path=path,body=body,initial=initial,raw=raw,auth=auth,initial_env=initial_env,wire_body=wire_body,difference=difference,status=response.status_code,response=response.json(),peers=effect,env=peers_api._ENV_ARQUIVO.read_text()))
    capture('proof-query-missing','GET','/api/peers/prova',auth=False)
    capture('pairing-query-missing','GET','/api/alcance/pareamento')
    for name,raw in [('truncated','{'),('array','[]')]:
        capture('delete-'+name,'DELETE','/api/peers/a',raw=raw)
        capture('enabled-'+name,'PUT','/api/peers/a/enabled',{'enabled':False},raw=raw)
    for method,path in [('POST','/api/peers'),('PUT','/api/peers/app-list'),('PUT','/api/peers/a/enabled'),('PUT','/api/peers/identificador')]:
        for kind,wire in [('missing',''),('array','[]'),('invalid','{'),('null','null'),('string','"text"')]:
            capture('body-'+path+'-'+kind,method,path,wire_body=wire,difference='rust-body-envelope')
    capture('announce-empty-body','POST','/api/peers/anunciar')
    capture('empty', 'GET', '/api/peers')
    for path in ['/api/peers','/api/peers/app-list','/api/peers/identificador','/api/peers/descobrir','/api/alcance']:
        capture('auth-'+path,'GET',path,auth=False)
    capture('malformed-list','GET','/api/peers',initial={'bad':'broken','app':{'app':{}},'z':{'base_url':'http://z','token':'abcdefghijk','enabled':False},'a':{'base_url':'http://a','token':123,'enabled':'yes','web_url':'https://a'}})
    for token in ['', 'a','12345678','123456789','á😀abcdéfghi']:
        capture('mask-'+token,'GET','/api/peers',initial={'a':{'base_url':'http://a','token':token}})
    for sid in ['a','a_b-9','a'*32,'a'*33,'Casa','a\n','','a b',1,[1]]:
        capture('id-'+str(sid),'POST','/api/peers',{'id':sid,'base_url':'http://example','token':'secret'})
    for base,token,web in [('ftp://x','t',None),('http://x','',None),(7,'t',None),('http://x',7,None),('http://x','t',7),('  https://x/// ',' t ',' https://w '),('http://127.0.0.1','t',None)]:
        capture('fields-'+str((base,token,web)),'POST','/api/peers',{'id':'a','base_url':base,'token':token,'web_url':web})
    original={'a':{'base_url':'http://old','token':'old','web_url':'https://web','enabled':False,'unknown':[1],'app':{'id':'one','pos':0}}}
    capture('upsert-preserves','POST','/api/peers',{'id':'a','base_url':'http://new','token':'new'},initial=original)
    capture('remove-preserves-app','DELETE','/api/peers/a',initial=original)
    capture('remove-unknown','DELETE','/api/peers/missing',initial=original)
    for value in [True,False,0,'false',None]:
        capture('enabled-'+str(value),'PUT','/api/peers/a/enabled',{'enabled':value},initial=original)
    capture('token','GET','/api/peers/a/token',initial=original)
    capture('token-missing','GET','/api/peers/missing/token',initial=original)
    capture('app-empty','GET','/api/peers/app-list')
    empty_rev=peers._revisao([])
    machine={'id':'srv-one','label':'Máquina','address':'https://one','token':'t','disabled':False,'invite':False}
    capture('app-write','PUT','/api/peers/app-list',{'rev':empty_rev,'servers':[machine]},initial={'a':{'base_url':'https://one','token':'owner'}})
    capture('app-conflict','PUT','/api/peers/app-list',{'rev':'old','servers':[]})
    capture('app-rev-type','PUT','/api/peers/app-list',{'rev':3,'servers':[]})
    for field,value in [('id',''),('id','x'*65),('address','ftp://x'),('label',2),('disabled',0),('invite','false'),('lan',{'url':'x'}),('token',None)]:
        capture('app-invalid-'+field,'PUT','/api/peers/app-list',{'rev':empty_rev,'servers':[{**machine,field:value}]})
    capture('app-duplicate','PUT','/api/peers/app-list',{'rev':empty_rev,'servers':[machine,machine]})
    capture('app-not-list','PUT','/api/peers/app-list',{'rev':empty_rev,'servers':{}})
    capture('ping','GET','/api/peers/ping',auth=False)
    for challenge in ['a'*15,'a'*16,'á'*16,'a'*128,'a'*129]:
        capture('proof-'+str(len(challenge))+'-'+challenge[0],'GET','/api/peers/prova?desafio='+challenge,auth=False)
    capture('identity','GET','/api/peers/identificador')
    for value in ['novo','',1,'A\n']:
        capture('identity-write-'+str(value),'PUT','/api/peers/identificador',{'identificador':value},initial_env='# comentário\nCP_SERVER_ID=fixture\nOTHER=yes\n')
    capture('check-no-url','GET','/api/peers/check?id=remote')
    capture('check-no-id','GET','/api/peers/check?url=http://remote')
    capture('check-ok','GET','/api/peers/check?url=http://remote&id=remote')
    capture('check-other','GET','/api/peers/check?url=http://remote&id=other')
    capture('discovery-unavailable','GET','/api/peers/descobrir')
    capture('reach','GET','/api/alcance')
    capture('pair-unknown','GET','/api/alcance/pareamento?endereco=other')
    from app import doctor
    doctor_cases=[]
    for name, changes in [
        ('healthy',{}),('factory-token',{'token':'change-me'}),('offline',{'port_open':False}),
        ('hung',{'latency':None}),('slow',{'latency':4.1}),('restarts-unknown',{'restarts':None}),
        ('restarts',{'restarts':['2026-10-28T10:00:00']}),('no-mux',{'mux':False}),
        ('codex-only',{'claude':False}),('no-agent',{'claude':False,'others':[]}),
        ('logged-out',{'logged_in':False}),('no-tailscale',{'tailscale':'ausente'}),
        ('tailscale-logout',{'tailscale_login':''}),('no-public-url',{'public_url':''}),
        ('lan-down',{'lan':False})]:
        values=dict(token='contract-owner',port_open=True,latency=.05,restarts=[],mux=True,
                    claude=True,others=['codex'],logged_in=True,tailscale='instalado',
                    tailscale_login='logado',public_url='https://fixture.invalid',lan=True)
        values.update(changes)
        settings.auth_token=values['token'];settings.public_url=values['public_url']
        doctor._WIN=False
        doctor._porta_responde=lambda _:values['port_open']
        doctor._tempo_resposta=lambda _:values['latency']
        doctor._reinicios_automaticos=lambda:values['restarts']
        doctor._binario=lambda n: n if (n=='tmux' and values['mux']) or (n=='claude' and values['claude']) or n in values['others'] else None
        doctor._claude_logado=lambda:values['logged_in']
        doctor._tailscale=lambda:(values['tailscale'],values['tailscale_login'])
        doctor._lan_responde=lambda _:values['lan']
        doctor_cases.append({'name':name,'input':values,'lines':[list(line) for line in doctor.diagnosticar(settings)]})
    output.parent.mkdir(parents=True,exist_ok=True)
    output.with_name('doctor-cases.json').write_text(json.dumps({'source_commit':commit,'cases':doctor_cases},ensure_ascii=False,indent=2)+'\n')
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/peers/generate.py --reference <checkout-Python> --output backend/tests/fixtures/contract/peers/cases.json','cases':cases},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(cases)} contratos capturados do Python {commit}')
