"""Caracteriza valores externos especiais sem alterar o corpus de checagens comuns."""
import json
from pathlib import Path
import subprocess

source=Path(__file__).with_name("generate_checks.py")
namespace={"__file__":str(source)}
exec(compile(source.read_text().split("for cli, path, content in [",1)[0],str(source),"exec"),namespace)
run=namespace["run"]
values=[None,False,True,0,1,"",[],["a"],{}, {"x":1},float("nan"),float("inf"),"\ud800", "\u001cfixture\u001f"]
for index,value in enumerate(values):
    run(f"claude-model-value-{index}","_modelo_padrao",files={".claude/settings.json":json.dumps({"model":value})})
    run(f"claude-mcp-value-{index}","_mcp",files={".claude.json":json.dumps({"mcpServers":value})})
    run(f"pi-model-value-{index}","_modelo_padrao","pi",files={".pi/agent/settings.json":json.dumps({"defaultProvider":value,"defaultModel":"fixture"})})
for cli in ("codex","kimi"):
    field="model" if cli=="codex" else "default_model"
    folder=".codex" if cli=="codex" else ".kimi-code"
    for value in ("nan","inf","-inf","true","0","1","[]",'["x"]','{x=1}'):
        run(f"{cli}-model-{value}","_modelo_padrao",cli,files={folder+"/config.toml":field+"="+value})
root=namespace["ROOT"]
Path(__file__).with_name("values.json").write_text(json.dumps(dict(
    source_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=root,text=True).strip(),
    command="python backend/tests/fixtures/contract/harness-health/generate_values.py",
    cases=namespace["cases"]),ensure_ascii=True,indent=2)+"\n")
print(f"{len(namespace['cases'])} valores externos capturados")
