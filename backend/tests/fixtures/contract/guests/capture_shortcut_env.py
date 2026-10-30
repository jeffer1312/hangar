"""Ambiente gráfico original, sem consultar o gerenciador de serviços da máquina."""
import ast
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
node=next(n for n in ast.parse(Path('app/api.py').read_text()).body if isinstance(n,ast.FunctionDef) and n.name=='_shortcut_env')
rows=[]
for windows in [False,True]:
    for mode in ['filled','empty_inherited','existing','failed','timeout','nonzero']:
        env={'KEEP':'fixture'}
        if mode=='empty_inherited':env['DISPLAY']=''
        if mode=='existing':env.update(DISPLAY=':old',WAYLAND_DISPLAY='old',XDG_CURRENT_DESKTOP='old',HYPRLAND_INSTANCE_SIGNATURE='old')
        raw='DISPLAY=:new\nWAYLAND_DISPLAY=wayland-fixture\nXDG_CURRENT_DESKTOP=fixture\nHYPRLAND_INSTANCE_SIGNATURE=fixture\nOTHER=ignored\nDISPLAY=:last\n'
        calls=[]
        def run(args,**kwargs):
            calls.append([args,kwargs])
            if mode=='failed':raise OSError('fixture')
            if mode=='timeout':raise subprocess.TimeoutExpired('fixture',3)
            return SimpleNamespace(stdout=raw,returncode=7 if mode=='nonzero' else 0)
        ns=dict(os=SimpleNamespace(name='nt' if windows else 'posix',environ=env),
                subprocess=SimpleNamespace(run=run,SubprocessError=subprocess.SubprocessError),
                _DISPLAY_VARS=('DISPLAY','WAYLAND_DISPLAY','XDG_CURRENT_DESKTOP','HYPRLAND_INSTANCE_SIGNATURE'))
        exec(compile(ast.Module(body=[node],type_ignores=[]),'app/api.py','exec'),ns)
        rows.append(dict(windows=windows,mode=mode,env=env,stdout=raw,expected=ns['_shortcut_env'](),calls=calls))
result=dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            command='cd backend && uv run python tests/fixtures/contract/guests/capture_shortcut_env.py',
            isolation='Função original extraída; ambiente e subprocesso injetados. Nenhum systemctl executado. Windows é ramo simulado.',rows=rows)
Path('tests/fixtures/contract/guests/shortcut-env.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n',encoding='utf-8')
print(f'{len(rows)} ambientes capturados')
