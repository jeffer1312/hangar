"""Executa o instalador Python com efeitos externos controlados, sem instalar programas."""
import ast
import asyncio
import json
import logging
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
source = ast.parse(subprocess.check_output(["git", "show", "d290182f567812c92b75709a5cd814a089b4ea98:backend/app/harness_install.py"], cwd=ROOT, text=True))
names = {"_TETO_LOG", "_TIMEOUT", "ETAPAS"}
nodes = [node for node in source.body if isinstance(node,ast.ClassDef) or
         isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id in names for t in node.targets)]
logger = logging.getLogger("capture-install")
logger.addHandler(logging.NullHandler())
logger.propagate = False
ns = dict(asyncio=asyncio,os=os,_log=logger,
          diag=SimpleNamespace(rastrear=lambda _:lambda f:f,registrar=lambda *a,**k:None),
          comandos=lambda:{"codex":"npm install -g @openai/codex"},MANUAL={"codex":"https://github.com/openai/codex"})
exec(compile(ast.Module(body=nodes,type_ignores=[]),"original-harness-install","exec"),ns)
cases=[]
for name, spec in [
    ("success",{}),("command-nonzero",{"codes":[7]}),
    ("command-error",{"runner_error":"fixture"}),("missing-cli",{"installed":False}),
    ("missing-card",{"card_missing":True}),("wrapper-nonzero",{"codes":[0,3]}),
    ("wrapper-still-missing",{"wrapper":False}),("wrapper-unknown",{"wrapper":None}),
    ("wrapper-unavailable",{"wrapper_command_error":"bash ausente"}),
    ("windows-wrapper-ok",{"windows":True}),
    ("windows-wrapper-repaired",{"windows":True,"wrapper_before":False}),
    ("windows-wrapper-still-missing",{"windows":True,"wrapper":False}),
    ("windows-wrapper-unknown",{"windows":True,"wrapper":None}),
    ("repairs-ordered",{"repairs":["first","second"]}),
    ("repair-first-fails",{"repairs":["first","second","third"],"repair_error":"first"}),
    ("repair-last-fails",{"repairs":["first","second"],"repair_error":"second"}),
    ("windows-wrapper-fails",{"windows":True,"wrapper_before":False,"repair_error":"wrapper"}),
    ("log-limit",{"output":"\n".join(f"line-{n}" for n in range(403))}),
    ("log-splitlines",{"output":"a\rb\r\nc\v\fd\x1ce\x1df\x1eg\x85h\u2028i\u2029"}),
]:
    calls=[]
    codes=iter(spec.get("codes",[0,0]))
    wrapper_values=iter([spec.get("wrapper_before",spec.get("wrapper",True)),spec.get("wrapper",True)])
    def runner(argv,**kwargs):
        calls.append({"run":argv,"timeout":kwargs["timeout"],"cwd":str(kwargs.get("cwd",""))})
        if "runner_error" in spec: raise OSError(spec["runner_error"])
        if "output" in spec: kwargs["log"](spec["output"])
        return SimpleNamespace(returncode=next(codes,0))
    def repair(key):
        calls.append({"repair":key})
        if key==spec.get("repair_error"): raise ValueError("conserto recusado")
        return "feito"
    def command():
        if "wrapper_command_error" in spec:raise ValueError(spec["wrapper_command_error"])
        return ["fixture-wrapper"]
    card={"id":"codex","instalado":spec.get("installed",True),"itens":[{"conserto":key} for key in spec.get("repairs",[])]}
    ns.update(atualizar=SimpleNamespace(_rodar=runner),_resolver=lambda argv:argv,
        harness_saude=SimpleNamespace(_E_WINDOWS=spec.get("windows",False),_REPO="fixture-repo",TIMEOUT_INSTALADOR=900,
            esquecer_versao=lambda cli:calls.append({"invalidate":cli}),
            diagnosticar=lambda:[] if spec.get("card_missing") else [card],
            consertar=repair,cmd_instalador=command,
            _wrapper=lambda cli:{"ok":next(wrapper_values,spec.get("wrapper",True)),"params":{"lista":"bash"}}))
    service=ns["Instalador"]()
    service._estado=service._zerado(fase="rodando",harness="codex",etapa="comando",passo=1)
    with patch.dict(os.environ,{"PATH":"fixture-bin"},clear=True):
        service._executar("codex",["fixture-install"])
    cases.append(dict(name=name,input=spec,expected=dict(status=service.status(),calls=calls)))
Path(__file__).with_name("execution.json").write_text(json.dumps(dict(
    source_commit='d290182f567812c92b75709a5cd814a089b4ea98',
    command="python backend/tests/fixtures/contract/harness-install/generate_execution.py",cases=cases,
),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} cenários do instalador capturados")
