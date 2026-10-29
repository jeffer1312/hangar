"""Captura Python isolada, sem lifespan nem servidor: uv run python tests/fixtures/contract/guests/capture_diary.py."""
import base64
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))

with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=str(root / 'claude'),
                      CP_AUTH_TOKEN='owner', CP_SYNC='1', CP_SYNC_DATA=str(root / 'vault.json'),
                      CP_SYNC_BOOTSTRAP='bootstrap', CP_SYNC_SESSION_SECRET='fixture-secret',
                      CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import guest_users, guest_users_api, guest_user_gate, sync, runtime_config
    guest_users._path_override = root / 'guests.json'
    runtime_config._backend_config_base = lambda: root / 'claude'
    (root / 'projects').mkdir()
    app = FastAPI()
    app.include_router(guest_users_api.router)
    app.include_router(sync.sync_router)
    app.include_router(sync.sync_admin_router)
    app.add_middleware(guest_user_gate.GuestUserGate)
    c = TestClient(app, raise_server_exceptions=False)
    from app import diag
    events=[]
    def record(evento,nivel='ok',**fields):
        events.append({'evento':evento,'nivel':nivel,**{k:v for k,v in fields.items() if k!='ms' and v is not None}})
    diag.registrar=record
    rows = []
    def normalized(value):
        return json.loads(json.dumps(value, ensure_ascii=False).replace(scratch, '⟦ROOT⟧'))
    def call(label, method, path, body=None, token='owner', cookie=None):
        events.clear()
        c.cookies.clear()
        headers = {'Authorization': 'Bearer ' + token} if token is not None else {}
        if cookie:
            headers['Cookie'] = 'cp_sync=' + cookie
        r = c.request(method, path, json=body, headers=headers)
        try:
            data = r.json()
        except ValueError:
            data = r.text
        rows.append(normalized({'name':label,'method':method,'path':path,'body':body,'token':token,
            'cookie':cookie,'status':r.status_code,'response':data,
            'now':sync.time.time(),'events':list(events),'set_cookie':r.headers.get('set-cookie'),
            'guests':json.loads((root/'guests.json').read_text()) if (root/'guests.json').exists() else None,
            'vault':json.loads((root/'vault.json').read_text()) if (root/'vault.json').exists() else None}))
        return r
    with patch('secrets.token_hex', lambda n: 'ab' * n), patch('secrets.token_urlsafe', lambda n: 'guest-token'), \
         patch('secrets.token_bytes', lambda n: b'x' * n), patch('app.sync.time.time', lambda: 2000000000):
        body = {'name':' Ana ','root':str(root/'projects')}
        call('dono', 'GET','/api/me')
        call('desconhecido','GET','/api/me',token='unknown')
        call('sem_raiz','POST','/api/guests',{'name':'Ana'})
        call('pasta_ausente','POST','/api/guests',{'name':'Ana','root':str(root/'missing')})
        call('criar','POST','/api/guests',body)
        call('convidado','GET','/api/me',token='guest-token')
        call('nao_administra','POST','/api/guests',body,token='guest-token')
        call('editar','POST','/api/guests/'+ 'ab'*8,{'root':str(root/'projects'),'sees_owner':True,'owner_sees':False})
        call('editar_ausente','POST','/api/guests/none',{'root':str(root/'projects')})
        call('apagar','POST','/api/guests/'+ 'ab'*8+'/delete')
        call('apagar_ausente','POST','/api/guests/'+ 'ab'*8+'/delete')
        call('revogado','GET','/api/me',token='guest-token')
        salt = base64.b64encode(b's'*16).decode()
        auth = base64.b64encode(b'a'*32).decode()
        blob = {'iv':base64.b64encode(b'i'*12).decode(),'data':base64.b64encode(b'd'*16).decode()}
        call('sync_inicial','GET','/api/sync/status',token=None)
        call('prelogin_sem_conta','GET','/api/sync/prelogin?user=owner',token=None)
        call('bootstrap_errado','POST','/api/sync/register',{'user':'owner','salt':salt,'auth_hash':auth,'bootstrap':'bad'},token=None)
        call('registrar','POST','/api/sync/register',{'user':'owner','salt':salt,'auth_hash':auth,'bootstrap':'bootstrap'},token=None)
        call('registrar_repetido','POST','/api/sync/register',{'user':'owner','salt':salt,'auth_hash':auth,'bootstrap':'bootstrap'},token=None)
        call('prelogin_dono','GET','/api/sync/prelogin?user=owner',token=None)
        call('prelogin_falso','GET','/api/sync/prelogin?user=missing',token=None)
        call('login_ruim','POST','/api/sync/login',{'user':'owner','auth_hash':'bad'},token=None)
        for i in range(11):
            call(f'login_repetido_{i}','POST','/api/sync/login',{'user':'owner','auth_hash':'bad'},token=None)
        with patch('app.sync.time.time',lambda:2000000400):
            r=call('login','POST','/api/sync/login',{'user':'owner','auth_hash':auth},token=None)
        cookie=r.cookies['cp_sync']
        call('cofre_vazio','GET','/api/sync/vault',token=None,cookie=cookie)
        call('cofre_gravar','PUT','/api/sync/vault',{'enc_blob':blob,'base_rev':0},token=None,cookie=cookie)
        call('cofre_conflito','PUT','/api/sync/vault',{'enc_blob':None,'base_rev':0},token=None,cookie=cookie)
        call('lista_vazia','GET','/api/sync/guests',token=None,cookie=cookie)
        account={'user':'ana','salt':salt,'auth_hash':auth,'enc_blob':blob,'admin_blob':blob}
        call('guest_hub','POST','/api/sync/guests',account,token=None,cookie=cookie)
        call('lista_hub','GET','/api/sync/guests',token=None,cookie=cookie)
        call('nome_dono','POST','/api/sync/guests',account|{'user':'owner'},token=None,cookie=cookie)
        call('nome_vazio','POST','/api/sync/guests',account|{'user':' '},token=None,cookie=cookie)
        call('nome_barra','POST','/api/sync/guests',account|{'user':'a/b'},token=None,cookie=cookie)
        r=call('login_guest','POST','/api/sync/login',{'user':'ana','auth_hash':auth},token=None)
        gc=r.cookies['cp_sync']
        call('guest_cofre','GET','/api/sync/vault',token=None,cookie=gc)
        call('guest_nao_admin','GET','/api/sync/guests',token=None,cookie=gc)
        call('remove_hub','POST','/api/sync/guests/ana/delete',token=None,cookie=cookie)
        call('cookie_revogado','GET','/api/sync/vault',token=None,cookie=gc)
        call('remove_hub_ausente','POST','/api/sync/guests/ana/delete',token=None,cookie=cookie)
        call('logout','POST','/api/sync/logout',token=None)
        call('desativar','POST','/api/sync/setup/disable')
        call('reativar','POST','/api/sync/setup',{})
    (root/'vault.json').write_text('{')
    events.clear()
    with patch('app.sync.time.time',lambda:2000000000):
        r=c.post('/api/sync/login',json={'user':'owner','auth_hash':auth})
    rows.append({'name':'cofre_ilegivel_diario','method':'POST','path':'/api/sync/login','body':{'user':'owner','auth_hash':auth},'token':None,'cookie':None,'now':2000000000,'before_vault_raw':'{','status':r.status_code,'response':r.text,'events':list(events)})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_diary.py',
        'comparison':'evento, nível, código e detalhe na ordem; duração não determinística excluída; identidade do processo e operação são do transporte do diário',
        'rows':[row for row in rows if row['path'].startswith('/api/sync/')]}
    (HERE/'diary.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(result["rows"])} chamadas com diário capturadas')
