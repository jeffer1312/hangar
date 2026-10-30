"""Captura a montagem original dos cards, isolando os diagnósticos de cada item."""
import ast
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[5]
source=ast.parse(subprocess.check_output(["git", "show", "9e5e5d4e3fc280f43e17d10da99db662fafe0877:backend/app/harness_saude.py"], cwd=ROOT, text=True))
ns=dict(Path=Path,ThreadPoolExecutor=ThreadPoolExecutor)
exec(compile(ast.Module(body=[n for n in source.body if isinstance(n,ast.FunctionDef) and n.name in {"_item","diagnosticar"}],type_ignores=[]),"original-health-aggregate","exec"),ns)
cases=[]
for name,spec in [
    ("nothing",{}),("empty-version",{"versions":{"claude":"","codex":""}}),
    ("versions-without-config",{"versions":{key:"fixture" for key in ("claude","codex","pi","omp","kimi")}}),
    ("all-folders",{"directories":[".codex",".pi/agent",".omp/agent",".kimi-code"]}),
    ("codex-custom-without-default",{"directories":["custom-codex"],"codex":"custom-codex"}),
    ("omp-custom",{"directories":["custom-omp"],"omp":"custom-omp"}),
    ("kimi-statusline",{"directories":[".kimi-code"],"statusline":True}),
    ("hooks-healthy",{"hooks":[True,True]}),
    ("hooks-null-first",{"hooks":[True,None,True]}),
    ("hooks-false-wins",{"hooks":[None,True,False,None]}),
    ("hooks-first-failure",{"hooks":[False,False]}),
]:
    counts={}
    with tempfile.TemporaryDirectory() as raw:
        home=Path(raw)
        for directory in spec.get("directories",[]):(home/directory).mkdir(parents=True,exist_ok=True)
        if spec.get("statusline"):(home/".kimi-code/statusline.js").write_text("fixture")
        accounts=[];hooks={}
        for index,ok in enumerate(spec.get("hooks",[])):
            path=home/f"account-{index}";path.mkdir();(path/"settings.json").write_text("{}")
            accounts.append(SimpleNamespace(path=str(path)));hooks[str(path)]={"source":f"hooks-{index}","ok":ok}
        def version(cli):counts[cli]=counts.get(cli,0)+1;return spec.get("versions",{}).get(cli)
        ns.update(_versao=version,list_config_dirs=lambda:accounts,_hooks_claude=lambda p:hooks[str(p)],
            _plugins_claude=lambda:{"source":"plugins"},_contas_claude=lambda:{"source":"accounts"},
            _fullscreen_claude=lambda:{"source":"fullscreen-claude"},_card_tmux=lambda:{"source":"tmux"},
            _raiz_agente=lambda cli:home/spec.get("omp",".omp/agent"),kimi_home=lambda:home/".kimi-code",
            _hooks_codex=lambda:{"source":"hooks-codex"},_hooks_kimi=lambda p:{"source":"hooks-kimi"})
        for function in ("_credenciais","_wrapper","_extensoes","_fullscreen","_mcp","_modelo_padrao"):
            ns[function]=lambda cli,function=function:{"source":function+":"+cli}
        ns["_ponte_skills"]=lambda cli,home:{"source":"_ponte_skills:"+cli}
        with patch.dict(os.environ,{"HOME":raw,"USERPROFILE":raw,"CODEX_HOME":str(home/spec.get("codex",".codex"))},clear=True):result=ns["diagnosticar"]()
    cases.append(dict(name=name,input=spec,expected=dict(cards=result,version_calls=counts)))
Path(__file__).with_name("aggregate.json").write_text(json.dumps(dict(
    source_commit='9e5e5d4e3fc280f43e17d10da99db662fafe0877',
    command="python backend/tests/fixtures/contract/harness-health/generate_aggregate.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} montagens de cards capturadas")
