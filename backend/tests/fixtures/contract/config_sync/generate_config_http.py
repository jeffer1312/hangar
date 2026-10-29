"""Executa os handlers originais de configuração com FastAPI, sem lifespan do servidor."""
import ast
import asyncio
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

BACKEND=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(BACKEND))
from fastapi import FastAPI, HTTPException, Request
from fastapi.testclient import TestClient
from pytest import MonkeyPatch
from app import config, runtime_config as rc, pensamento, computer_control as cc
from app.mensagens import erro

names={'get_config','_somente_leitura','patch_config'}
nodes=[n for n in ast.parse((BACKEND/'app/api.py').read_text()).body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef)) and n.name in names]
for node in nodes:node.decorator_list=[]
code=compile(ast.Module(body=nodes,type_ignores=[]),'config-api','exec')
cases=[]
for method,raw in [('GET',''),('HEAD',''),('POST',''),('POST',' '),('POST','{}'),('PATCH','{"editor":"vim"}'),('POST','[]'),('POST','null'),('POST','false'),('POST','"texto"'),('POST','{'),('POST','{"mostrar_pensamento":true}'),('POST','{"upload_retention_days":"inválido"}'),('POST','{"claude_function_hooks":false}'),
                   ('POST','{"editor":"\\ud800"}'),('POST','{"editor":"\\u0000"}'),
                   ('POST','{"upload_retention_days":NaN}'),('POST','{"upload_retention_days":Infinity}'),
                   ('POST','{"unknown":NaN}'),('POST','{"unknown":"\\ud800"}'),
                   ('POST','{"editor":null}'),('POST','{"mostrar_pensamento":1}')]:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(os.environ,{'HOME':folder,'USERPROFILE':folder},clear=True):
        home=Path(folder);base=home/'.claude';base.mkdir()
        settings=config.Settings(_env_file=None,scan_roots='',projects_dir=base/'projects',sync_data=home/'.hangar/sync-vault.json')
        mp.setattr(config,'settings',settings);mp.setattr(rc,'settings',settings)
        mp.setattr(rc,'_backend_config_base',lambda:base);mp.setattr(rc,'_cache',None)
        mp.setattr(pensamento,'compartilhado',lambda:base)
        mp.setattr(cc,'_main_file',lambda:home/'.claude.json');mp.setattr(cc,'_parked_file',lambda:home/'.hangar/computer-control.json')
        calls=[]
        ns={'Request':Request,'HTTPException':HTTPException,'runtime_config':rc,'settings':settings,'asyncio':asyncio,'erro':erro,
            'variaveis_env':config.variaveis_env,'diag':SimpleNamespace(VERSAO_EM_EXECUCAO='fixture-running'),
            '_painel_disponivel':lambda:True,'_traducao_pensamento_disponivel':lambda:False,'_origem_do_terminal_ok':lambda request:True,
            'plugin_bridge':SimpleNamespace(publish_address=lambda:calls.append('publish')),'_log':logging.getLogger('fixture')}
        exec(code,ns)
        app=FastAPI();app.add_api_route('/api/config',ns['get_config'],methods=['GET']);app.add_api_route('/api/config',ns['patch_config'],methods=['POST','PATCH'])
        with TestClient(app,raise_server_exceptions=False) as client:
            response=client.request(method,'/api/config',content=raw,headers={'content-type':'application/json'})
        try:body=response.json()
        except ValueError:body=response.text
        body=json.loads(json.dumps(body).replace(folder,'⟦HOME⟧'))
        files={str(p.relative_to(home)).replace('\\','/'):p.read_text().replace(folder,'⟦HOME⟧') for p in home.rglob('*') if p.is_file()}
        cases.append({'method':method,'raw':raw,'status':response.status_code,'body':body,'calls':calls,'allow':response.headers.get('allow'),'files':files})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/config_sync/generate_config_http.py',
        'injections':['autenticação comparada nas provas do porteiro','versão fixture-running','terminal disponível','tradução indisponível','origem do dono aceita','publicação do plugin registrada'], 'cases':cases}
Path(__file__).with_name('config-http.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
