"""Executa a guarda original de rodada sem lançar threads ou instaladores."""
import ast
import asyncio
import json
import logging
from pathlib import Path
import subprocess
from types import SimpleNamespace

ROOT=Path(__file__).resolve().parents[5]
source=ast.parse(subprocess.check_output(["git", "show", "d290182f567812c92b75709a5cd814a089b4ea98:backend/app/harness_install.py"], cwd=ROOT, text=True))
nodes=[node for node in source.body if isinstance(node,ast.ClassDef) or isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id in {"ETAPAS","_TIMEOUT","_TETO_LOG"} for t in node.targets)]
class Task:
    def __init__(self): self.failure=None;self.cancelled=False
    def add_done_callback(self,callback): self.callback=callback
    def exception(self):
        if self.cancelled:raise asyncio.CancelledError()
        return self.failure
created=[]
def create_task(work):
    task=Task();created.append(task);return task
logger=logging.getLogger("capture-concurrency");logger.addHandler(logging.NullHandler());logger.propagate=False
ns=dict(asyncio=SimpleNamespace(create_task=create_task,to_thread=lambda *args:None,CancelledError=asyncio.CancelledError),
    _log=logger,_COMANDOS={"codex":["fixture"],"pi":["fixture-pi"],"kimi":None},
    comandos=lambda:{"codex":"fixture","pi":"fixture-pi"},MANUAL={},
    diag=SimpleNamespace(rastrear=lambda _:lambda f:f,registrar=lambda *args,**kwargs:None))
exec(compile(ast.Module(body=nodes,type_ignores=[]),"original-install-concurrency","exec"),ns)
service=ns["Instalador"]();cases=[]
def state(name,error=None):
    cases.append(dict(name=name,status=service.status(),created=len(created),error=error))
async def begin(cli,name):
    error=None
    try:await service.iniciar(cli)
    except Exception as failure:error=dict(type=type(failure).__name__,message=str(failure))
    state(name,error)
state("initial")
asyncio.run(begin("unknown","unknown-cli"))
asyncio.run(begin("kimi","unsupported-cli"))
asyncio.run(begin("codex","first"));old=created[-1]
asyncio.run(begin("codex","same-running"))
asyncio.run(begin("pi","other-running"))
service._pub(fase="pronto",ok=True)
asyncio.run(begin("codex","new-after-phase-ready"))
service._encerrou(old);state("old-callback-noop")
created[-1].failure=RuntimeError("fixture")
service._encerrou(created[-1]);state("current-error")
asyncio.run(begin("pi","new-provider"))
created[-1].cancelled=True
service._encerrou(created[-1]);state("cancelled-current")
service._encerrou(created[-1]);state("ready-callback-noop")
Path(__file__).with_name("concurrency.json").write_text(json.dumps(dict(
    source_commit='d290182f567812c92b75709a5cd814a089b4ea98',
    command="python backend/tests/fixtures/contract/harness-install/generate_concurrency.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} estados de concorrência capturados")
