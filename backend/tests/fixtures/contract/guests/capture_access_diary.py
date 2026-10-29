"""Captura o limitador de repetição do diário original, isolado das rotas e do transporte."""
import json, os, sys, tempfile, subprocess
from pathlib import Path
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='fixture',CP_SYNC='1',CP_SYNC_DATA=str(Path(scratch)/'vault.json'),CP_SYNC_SESSION_SECRET='fixture')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from app import auth,diag
    rows=[];events=[]
    diag.registrar=lambda evento,nivel='ok',**fields:events.append(dict(evento=evento,nivel=nivel,**fields))
    def call(ip,reason):
        events.clear();auth.registrar_acesso(ip,'cookie',reason,dominio='sync.sessao')
        rows.append(dict(ip=ip,reason=reason,events=list(events)))
    auth._diag_recusas.clear()
    call('fixture-repeat','cookie_ausente');call('fixture-repeat','cookie_ausente')
    call('fixture-repeat','sessao_invalida_ou_expirada');call('fixture-repeat',None);call('fixture-repeat',None)
    for i in range(513):call(f'fixture-{i}','cookie_ausente')
    call('fixture-0','cookie_ausente');call('fixture-1',None);call('fixture-2',None)
    out=dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),command='cd backend && uv run python tests/fixtures/contract/guests/capture_access_diary.py',isolation='auth.registrar_acesso original; sem HTTP, contador de autenticação nem transporte do diário',rows=rows)
    (HERE/'access-diary.json').write_text(json.dumps(out,ensure_ascii=True,indent=2)+'\n')
