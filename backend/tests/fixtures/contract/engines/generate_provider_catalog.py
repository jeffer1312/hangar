"""Captura o catálogo original com logins e sondas sintéticos, sem backend em execução."""
import ast
import asyncio
import json
from pathlib import Path
import subprocess
import sys
from types import ModuleType, SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
node=next(n for n in ast.parse((ROOT/'backend/app/api.py').read_text()).body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_session_provider_catalog')
choose=ModuleType('session_defaults')
exec((ROOT/'backend/app/session_defaults.py').read_text(),choose.__dict__)
app=ModuleType('app');app.session_defaults=choose;sys.modules['app']=app
async def main():
    cases=[]
    for name,available,remembered,logins,codex,engines in [
        ('none',[],None,[],[],False),
        ('claude',['claude'],None,[],[],False),
        ('codex',['codex'],None,[],[],False),
        ('ordered',['pi','omp','kimi'],None,[],[],False),
        ('remembered',['pi','omp','kimi'],'kimi',[],[],False),
        ('claude_connected',['claude','codex'],None,[True],['connected'],False),
        ('codex_connected',['claude','codex'],None,[False],['connected'],False),
        ('remembered_disconnected',['claude','codex'],'claude',[False],['connected'],False),
        ('remembered_without_connected',['claude','codex'],'codex',[False],['disconnected'],False),
        ('engine_connects_claude',['claude','codex'],None,[False],['connected'],True),
        ('unknown_claude',['claude','codex'],None,[None],['connected'],False),
        ('failed_claude',['claude','codex'],None,'error',['connected'],False),
        ('mixed_codex',['claude','codex'],'codex',[True],['disconnected','unavailable'],False),
        ('unknown_remembered',['claude','pi'],'absent',[],[],False),
    ]:
        probes={p:{'disponivel':p in available,'motivo':None if p in available else 'nao_encontrado'} for p in ['claude','codex','pi','omp','kimi']}
        def reads(_):
            if logins=='error': raise ValueError('fixture')
            return [SimpleNamespace(loggedIn=state) for state in logins]
        app.conta_estado=SimpleNamespace(logins=reads)
        async def codex_states(): return [(None,{'status':state}) for state in codex]
        ns={'asyncio':asyncio,'cli_probe':SimpleNamespace(sondar_providers=lambda:probes),'list_config_dirs':lambda _:[],
            '_log':SimpleNamespace(warning=lambda *_args,**_kwargs:None),'engines':SimpleNamespace(listar=lambda:{'fixture':{}} if engines else {}),
            '_codex_auth_states':codex_states,'runtime_config':{'last_session_provider':remembered}}
        exec(compile(ast.Module(body=[node],type_ignores=[]),'original-provider-catalog','exec'),ns)
        cases.append({'name':name,'available':available,'remembered':remembered,'logins':logins,'codex':codex,'engines':engines,'result':await ns['_session_provider_catalog']()})
    Path(__file__).with_name('provider-catalog.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'python backend/tests/fixtures/contract/engines/generate_provider_catalog.py','cases':cases},ensure_ascii=False,indent=2)+'\n')
asyncio.run(main())
