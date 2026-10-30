"""Executa a leitura original do estado Codex em diretórios temporários."""
import ast
import asyncio
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace

ROOT=Path(__file__).resolve().parents[5]
ns=dict(Path=Path,asyncio=asyncio,copy=copy,hashlib=hashlib,json=json,CodexNativo=object,time=SimpleNamespace(time=lambda:1000.0))
exec(compile((ROOT/"backend/app/codex_msgs.py").read_text(),"original-codex-messages","exec"),ns)
files=ast.parse((ROOT/"backend/app/codex_arquivos.py").read_text())
exec(compile(ast.Module(body=[n for n in files.body if isinstance(n,ast.FunctionDef) and n.name in {"ler","json_obj"}],type_ignores=[]),"original-codex-read","exec"),ns)
source=ast.parse((ROOT/"backend/app/codex_integracao.py").read_text())
cls=next(n for n in source.body if isinstance(n,ast.ClassDef) and n.name=="IntegracaoCodex")
cls.body=[n for n in cls.body if isinstance(n,ast.FunctionDef) and n.name in {"__init__","home","codex_home","raiz","status","_status","_resumo_skills"}]
nodes=[n for n in source.body if isinstance(n,ast.FunctionDef) and n.name=="_snapshot"]+[cls]
exec(compile(ast.Module(body=nodes,type_ignores=[]),"original-codex-status","exec"),ns)
cases=[]
for name,disk,memory in [
    ("missing",None,None),("broken","{",None),("nonobject","[]",None),
    ("ready",{"status":{"estado":"pronto","etapa":"feito","ultima_execucao":"2026-01-01"}},None),
    ("restart",{"status":{"estado":"executando","etapa":"velha"}},None),
    ("skills",{"skills":{"a":{"mode":"symlink"},"b":{"mode":"copy"},"c":{"mode":"native"},"d":{"mode":"other"},"e":4}},None),
    ("memory-newer",{"status":{"ultima_execucao":"2026-01-01"}},{"estado":"pronto","ultima_execucao":"2026-02-01"}),
    ("disk-newer",{"status":{"ultima_execucao":"2026-02-01"}},{"estado":"erro","ultima_execucao":"2026-01-01"}),
    ("running",None,{"estado":"executando","etapa":"trabalhando","etapa_desde":990.5}),
    ("running-clock-back",None,{"estado":"executando","etapa_desde":1005.0}),
    ("invalid-status",{"status":[]},None),("invalid-skills",{"skills":[]},None),
    ("structured-messages",{"status":{"etapa":{"codigo":"fixture","params":{"x":"y"},"texto":"valor"},"avisos":["aviso",None],"erros":[{"codigo":"fixture","params":{},"texto":"erro"}]}},None),
]:
    with tempfile.TemporaryDirectory() as raw:
        home=Path(raw);service=ns["IntegracaoCodex"](home,home/".codex")
        if disk is not None:
            service.raiz.mkdir(parents=True);(service.raiz/"estado.json").write_text(disk if isinstance(disk,str) else json.dumps(disk))
        if memory is not None:service._estado={**ns["_snapshot"](),**memory}
        try:expected={"ok":service.status()}
        except Exception as error:expected={"error":type(error).__name__,"message":str(error)}
        cases.append(dict(name=name,disk=disk,memory=memory,expected=expected))
Path(__file__).with_name("status.json").write_text(json.dumps(dict(
    source_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
    command="python backend/tests/fixtures/contract/codex-integration/generate_status.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} estados Codex capturados")
