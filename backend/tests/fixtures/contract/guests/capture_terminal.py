"""Executa o porteiro original do terminal sem criar PTY nem consultar o multiplexador."""
import asyncio
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_PUBLIC_URL='https://public.example:8443',CP_TERM_ORIGINS='https://extra.example')
    from app import termsock,guest_users,runtime_config
    from starlette.websockets import WebSocket
    runtime_config._backend_config_base=lambda:Path(scratch)
    cases=[(False,'https://invalid.example'),(True,'https://invalid.example'),(True,'http://fixture'),(True,None),
           (True,'https://public.example:8443'),(True,'https://extra.example'),(True,'https://peer.example:8443'),(True,'null')]
    rows=[]
    async def run():
        for guest,origin in cases:
            sent=[]
            async def send(message):sent.append(message)
            async def receive():return {'type':'websocket.connect'}
            headers=[(b'host',b'fixture')]+([(b'origin',origin.encode())] if origin is not None else [])
            scope={'type':'websocket','path':'/api/sessions/s0/term','headers':headers,'query_string':b'token=guest-token' if guest else b'token=owner',
                   'client':('127.0.0.1',1234),'scheme':'ws','server':('fixture',80),'root_path':''}
            token=guest_users.current.set(guest_users.Guest('guest','Ana','hash',scratch,True,True) if guest else None)
            try:
                with patch('app.tmux.has_session',lambda _:True),patch('app.termsock._peers_conhecidos',lambda:['https://peer.example:8443']):
                    result=await termsock._porta_de_entrada(WebSocket(scope,receive,send),'s0')
            finally:guest_users.current.reset(token)
            rows.append({'guest':guest,'origin':origin,'accepted':result is not None,'result':result,'sent':sent})
    asyncio.run(run())
    (HERE/'terminal.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_terminal.py','rows':rows},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} origens Python capturadas')
