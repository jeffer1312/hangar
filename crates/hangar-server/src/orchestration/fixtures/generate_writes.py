"""Congela as escritas HTTP da referência e os bytes do contrato resultante.

A versão devolvida é conferida contra o arquivo de cada execução, pois mtime vem do sistema.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(os.environ.get('HANGAR_ORQ_REFERENCE_ROOT') or Path(__file__).resolve().parents[5])
sys.path.insert(0, str(ROOT / 'backend'))
from fastapi.testclient import TestClient
from app import api, orq_context, orq_papeis, orq_politica, pair

api.settings.auth_token = 'fixture-owner'
api.app.dependency_overrides[api._transfer_guard] = lambda: None
api.app.dependency_overrides[api._transfer_check] = lambda: None
orq_context.identity = lambda name: 'claude:key'
orq_context.active_gid = lambda name: None
orq_politica._niveis = lambda *args: None
client = TestClient(api.app, raise_server_exceptions=False)
role = {'papel':'executor','provider':'claude','conta':'padrao','modelo':'opus','esforco':'high'}
initial = '# Contrato\n\n## Gates\n\n- Preserve esta instrução.\n'
requests = [
    ('POST','papel','',role),
    ('POST','papel',initial,role | {'sessao':'trabalho-t*','headless':False}),
    ('POST','papeis',initial,{'papeis':[role | {'vez':'1','janela':'60'}, role | {'vez':'2','headless':True}]}),
    ('POST','papeis',initial,{'papeis':[]}),
    ('POST','papel',initial,role | {'papel':'x|y'}),
    ('POST','papeis',initial,{'papeis':[role | {'janela':'9'}]}),
    ('POST','papel',initial,role | {'provider':'pi','headless':True,'esforco':''}),
    ('DELETE','papel',initial,{'papel':'ausente'}),
]
cases=[]
for method,suffix,content,body in requests:
    with tempfile.TemporaryDirectory() as scratch:
        home=Path(scratch);directory=home/'.claude'/'.hangar-pair'
        pair._pair_dir=lambda:directory
        pair.PairLink=lambda name:type('Link',(),{'get':lambda self:None})()
        orq_politica.caminho=lambda:home/'.hangar'/'orquestracao-contas.md'
        gid=orq_context.draft_gid('planner');path=directory/f'regras-{gid}.md'
        directory.mkdir(parents=True)
        if content:path.write_text(content,encoding='utf-8')
        body={**body,'mtime':path.stat().st_mtime if path.exists() else 0.0}
        response=client.request(method,f'/api/sessions/planner/orq/{suffix}',headers={'authorization':'Bearer fixture-owner'},json=body)
        output=response.json()
        version=output.pop('mtime',None)
        if version is not None:assert version==path.stat().st_mtime
        body['mtime']='FILE_MTIME'
        cases.append({'method':method,'path':f'/api/sessions/planner/orq/{suffix}','initial':content,'file':path.name,'body':body,'status':response.status_code,'output':output,'version':version is not None,'final':path.read_text(encoding='utf-8') if path.exists() else None})
print(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(ROOT),'rev-parse','HEAD'],text=True).strip(),'command':'HANGAR_ORQ_REFERENCE_ROOT=<source_commit> python crates/hangar-server/src/orchestration/fixtures/generate_writes.py','comparison':'mtime corresponde ao fstat do arquivo; demais campos e bytes são exatos','cases':cases},ensure_ascii=False,indent=2))
