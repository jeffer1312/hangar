"""Executa porteiro e GET de loop originais, com vida e registro de sessões isolados."""
import ast
import asyncio
import httpx2
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from fastapi import FastAPI,Depends
sys.path.insert(0,str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from app import loop as loop_mod,guest_users,guest_user_gate,auth
    guest_users._path_override=root/'guests.json'
    source=Path.cwd()/'app/api.py'
    node=next(n for n in ast.parse(source.read_text()).body if isinstance(n,ast.FunctionDef) and n.name=='loop_get')
    node.decorator_list=[];ns={'registry':SimpleNamespace(list=lambda:[]),'loop_mod':loop_mod}
    exec(compile(ast.Module(body=[node],type_ignores=[]),str(source),'exec'),ns)
    app=FastAPI();app.add_api_route('/api/sessions/{name}/loop',ns['loop_get'],methods=['GET'],dependencies=[Depends(auth.require_auth)])
    app.add_api_route('/api/sessions/{name}/then',lambda:{'unexpected':True},methods=['PUT'])
    app.add_api_route('/api/config',lambda:{'unexpected':True},methods=['GET'])
    app.add_middleware(guest_user_gate.GuestUserGate)
    rows=[]
    with patch.object(guest_users,'session_life',lambda _:None):
        guest,token=guest_users.create('Fixture',scratch,False,True);guest_users.claim('fixture',guest.id)
        async def capture():
            async with httpx2.AsyncClient(transport=httpx2.ASGITransport(app=app,raise_app_exceptions=False),base_url='http://testserver') as client:
                for method,path in [('GET','/api/sessions/fixture/loop'),('GET','/api/sessions/other/loop'),('PUT','/api/sessions/fixture/then'),('GET','/api/config'),('GET','/api/%73essions/fixture/loop'),('GET','/api/sessions/%66ixture/loop'),('GET','/api/sessions/fixture%2Fother/loop'),('GET','/api/sessions/%2566ixture/loop')]:
                    response=await client.request(method,path,headers={'Authorization':'Bearer '+token})
                    rows.append({'method':method,'path':path,'status':response.status_code,'response':response.json()})
        asyncio.run(capture())
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_gate.py','isolation':'ASGITransport decodifica o caminho uma vez (TestClient decodifica duas); GuestUserGate/guest_users/loop_get originais; registro vazio e session_life=None para não consultar tmux/sessões reais.','rows':rows}
    (Path(__file__).resolve().parent/'loop-gate.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} acessos de login ao loop capturados')
