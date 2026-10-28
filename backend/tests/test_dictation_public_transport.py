"""As portas Python conservam somente autorização e transporte do ditado."""

import pytest
from fastapi.testclient import TestClient
from starlette.responses import JSONResponse

from app import account_bridge, api, guest_users
from app.config import settings

@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(settings, 'auth_token', 'fixture-owner')
    monkeypatch.setattr(account_bridge, 'owner_mode', lambda: 'rust')
    return TestClient(api.app)

@pytest.mark.parametrize('method,path,body', [
    ('POST', '/api/dictation/transcribe?limpar=0', b'audio'),
    ('POST', '/api/ditado/relimpar', b'{"texto":"fala","estilo":"prosa"}'),
    ('GET', '/api/dictation/models?session=x', b''),
    ('GET', '/api/transcription/providers/status', b''),
    ('POST', '/api/transcription/providers/p/test', b'audio'),
])
def test_dictation_ports_forward_original_request(client, monkeypatch, method, path, body):
    seen=[]
    async def forward(request, *, guest):
        seen.append((request.method, request.url.path, request.url.query, await request.body(), guest, request.headers.get('x-filename')))
        return JSONResponse({'text':'Resposta Rust'},status_code=201)
    monkeypatch.setattr(account_bridge,'forward_stream',forward)
    response=client.request(method,path,content=body,headers={'Authorization':'Bearer fixture-owner','Content-Type':'application/json','X-Filename':'nota.m4a'})
    assert response.status_code==201
    assert response.json()=={'text':'Resposta Rust'}
    endpoint,_,query=path.partition('?')
    assert seen==[(method,endpoint,query,body,False,'nota.m4a')]


def test_dictation_guest_login_keeps_visibility_and_marks_transport(client, monkeypatch, tmp_path):
    monkeypatch.setattr(guest_users, '_path_override', tmp_path/'guests.json')
    monkeypatch.setattr(guest_users, 'session_life', lambda name: 't:1')
    guest_users._reset()
    guest, token=guest_users.create('visitante',str(tmp_path),False,True)
    guest_users.claim('visivel',guest.id)
    seen=[]
    async def forward(request, *, guest):
        seen.append(guest)
        return JSONResponse({'text':'fala'})
    monkeypatch.setattr(account_bridge,'forward_stream',forward)
    try:
        headers={'Authorization':f'Bearer {token}'}
        assert client.post('/api/ditado/relimpar',json={'texto':'fala','estilo':'prosa','session':'outra'},headers=headers).status_code==403
        assert seen==[]
        assert client.post('/api/ditado/relimpar',json={'texto':'fala','estilo':'prosa','session':'visivel'},headers=headers).status_code==200
        assert seen==[True]
        for path in ['/api/dictation/models?session=visivel','/api/transcription/providers/status']:
            assert client.get(path,headers=headers).status_code==403
        assert client.post('/api/transcription/providers/p/test',content=b'audio',headers=headers).status_code==403
    finally:
        guest_users._reset()


def test_dictation_python_mode_is_explicit_and_no_transport(client,monkeypatch):
    monkeypatch.setattr(account_bridge,'owner_mode',lambda:'python')
    async def unexpected(*args,**kwargs):
        pytest.fail('O modo Python não deve chamar outro motor.')
    monkeypatch.setattr(account_bridge,'forward_stream',unexpected)
    for path,body in [('/api/dictation/transcribe',b'audio'),('/api/ditado/relimpar',b'{"texto":"fala","estilo":"prosa"}')]:
        r=client.post(path,content=body,headers={'Authorization':'Bearer fixture-owner','Content-Type':'application/json'})
        assert r.status_code==503
        assert r.json()['detail']['code']=='dictation_need_rust_server'
    assert client.post('/api/dictation/transcribe',content=b'audio').status_code==401


def test_dictation_catalog_stays_unavailable_on_connect(client,monkeypatch):
    async def unexpected(*args,**kwargs):
        pytest.fail('O Connect não deve encaminhar o catálogo exclusivo do dono.')
    monkeypatch.setattr(account_bridge,'forward_stream',unexpected)
    connect=TestClient(api.app,base_url='http://testserver:8768')
    r=connect.get('/api/dictation/models?session=x',headers={'Authorization':'Bearer fixture-owner'})
    assert r.status_code==403
