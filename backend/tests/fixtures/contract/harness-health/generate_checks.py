"""Executa as checagens originais em arquivos temporários, sem CLIs ou contas reais."""
import ast
import json
import logging
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import tomllib
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[5]
functions = {"_item", "_ler_json", "_ler_toml", "_mcp", "_modelo_padrao", "_hooks_codex",
             "_conf_psmux", "_perfis_powershell", "_wrapper", "_raiz_agente", "_extensoes",
             "_abreviar_home", "_fullscreen", "_fullscreen_claude", "_hooks_claude", "_executavel_existe"}
constants = {"_EXTENSOES_PI", "_EXTENSOES_POR_CLI", "_HOOKS_CLAUDE"}
source = ast.parse((ROOT / "backend/app/harness_saude.py").read_text())
nodes = [node for node in source.body if
         (isinstance(node, ast.FunctionDef) and node.name in functions) or
         (isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id in constants for target in node.targets))]
ns = dict(json=json, os=os, Path=Path, shlex=shlex, shutil=shutil, tomllib=tomllib,
          _log=logging.getLogger("fixture"))
exec(compile(ast.Module(body=nodes, type_ignores=[]), "original-harness-checks", "exec"), ns)
cases = []

def run(name, kind, cli="claude", files=None, directories=None, links=None, programs=None, windows=False, env=None):
    files, directories, links = files or {}, directories or [], links or {}
    programs, env = programs or [], env or {}
    with tempfile.TemporaryDirectory() as raw:
        home = Path(raw)
        repo = home / "repo"
        repo.mkdir()
        expand = lambda text: text.replace("$ROOT", raw)
        for path in directories:
            (home / path).mkdir(parents=True, exist_ok=True)
        for path, text in files.items():
            target = home / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(expand(text))
        for path, target in links.items():
            link = home / path
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(expand(target))
        ns.update(_REPO=repo, _E_WINDOWS=windows,
                  _codex_dir=lambda _: home / ".codex", kimi_home=lambda: home / ".kimi-code",
                  omp_dirs=SimpleNamespace(agent_dir=lambda: home / ".omp/agent"),
                  contas=SimpleNamespace(compartilhado=lambda: home / ".claude"))
        with patch.dict(os.environ, {"HOME":raw,"USERPROFILE":raw,**{k:expand(v) for k,v in env.items()}}, clear=True), \
             patch.object(shutil,"which",side_effect=lambda name: "/fixture/"+name if name in programs else None):
            try:
                args = [] if kind in ("_hooks_codex", "_conf_psmux", "_fullscreen_claude") else [home / ".claude"] if kind == "_hooks_claude" else [cli]
                result = ns[kind](*args)
                expected = {"ok":str(result) if isinstance(result,Path) else result}
            except Exception as error:
                expected = {"error":type(error).__name__,"message":str(error)}
        expected = json.loads(json.dumps(expected,ensure_ascii=True).replace(raw,"$ROOT"))
        cases.append(dict(name=name,kind=kind,cli=cli,files=files,directories=directories,links=links,
                          programs=programs,windows=windows,env=env,expected=expected))

for cli, path, content in [
    ("claude",".claude.json",'{"mcpServers":{"z":{},"á":{},"a":{}}}'),
    ("codex",".codex/config.toml",'[mcp_servers.z]\n[mcp_servers.a]\n'),
    ("pi",".pi/agent/mcp.json",'{"mcpServers":{"z":{},"a":{}}}'),
    ("omp",".omp/agent/mcp.json",'{"mcpServers":{"z":{},"a":{}}}'),
    ("kimi",".kimi-code/config.toml",'[mcp_servers.z]\n[mcp_servers.a]\n'),
]:
    run(cli+"-mcp-missing","_mcp",cli)
    run(cli+"-mcp-present","_mcp",cli,{path:content})
    run(cli+"-mcp-invalid","_mcp",cli,{path:"{"})
for cli,path,content in [
    ("claude",".claude/settings.json",'{"model":"fixture"}'),
    ("codex",".codex/config.toml",'model="fixture"'),
    ("pi",".pi/agent/settings.json",'{"defaultProvider":"provider","defaultModel":"fixture"}'),
    ("omp",".omp/agent/settings.json",'{"defaultModel":"fixture"}'),
    ("kimi",".kimi-code/config.toml",'default_model="fixture"'),
]:
    run(cli+"-model-missing","_modelo_padrao",cli)
    run(cli+"-model","_modelo_padrao",cli,{path:content})
for name, content in [("missing",None),("broken","{"),("nonobject","[]"),("enabled",'{"enabled":true}'),("disabled",'{"enabled":false}'),("number",'{"enabled":1}')]:
    run("fullscreen-"+name,"_fullscreen","pi",{} if content is None else {".pi/agent/fullscreen-tui.json":content})
for name,content in [("missing",None),("invalid","{"),("other",'{"tui":"other"}'),("enabled",'{"tui":"fullscreen"}')]:
    run("fullscreen-claude-"+name,"_fullscreen_claude",files={} if content is None else {".claude/settings.json":content})
for name,content in [("missing",None),("invalid","{"),("empty",'{"hooks":{}}'),("shape",'{"hooks":[]}'),("events",'{"hooks":{"Stop":[{},{}],"PreToolUse":[{}],"Other":4}}')]:
    run("codex-hooks-"+name,"_hooks_codex",files={} if content is None else {".codex/hooks.json":content})
run("wrapper-no-shell","_wrapper")
run("wrapper-missing-no-bash","_wrapper",files={".bashrc":""})
run("wrapper-missing-with-bash","_wrapper",files={".bashrc":""},programs=["bash"])
run("wrapper-all-shells","_wrapper",files={".bashrc":"source /other/scripts/shell/claude.posix.sh",".zshrc":"source /other/scripts/shell/claude.posix.sh",".config/fish/functions/claude.fish":"fixture"})
run("wrapper-missing-zsh","_wrapper",files={".bashrc":"shell/claude.posix.sh",".zshrc":""},programs=["bash"])
run("wrapper-codex-no-launchers","_wrapper","codex",files={".bashrc":"shell/codex.posix.sh"},programs=["bash"])
run("wrapper-codex-complete","_wrapper","codex",files={".bashrc":"shell/codex.posix.sh",".local/bin/hangar-codex":"fixture",".local/bin/hangar-codex-tui":"fixture"})
run("wrapper-powershell-allhosts","_wrapper",files={"Documents/WindowsPowerShell/profile.ps1":r'. C:\other\scripts\shell\claude.ps1'},windows=True)
run("wrapper-powershell-missing","_wrapper",files={"Documents/PowerShell/Microsoft.PowerShell_profile.ps1":""},windows=True)
for name,files,env in [("default",{},{}),("first",{".psmux.conf":"",".tmux.conf":""},{}),("rc",{".psmuxrc":"",".tmux.conf":""},{}),("config",{".config/psmux/psmux.conf":""},{}),("override",{".psmux.conf":""},{"PSMUX_CONFIG_FILE":"$ROOT/custom.conf"})]:
    run("psmux-"+name,"_conf_psmux",files=files,env=env,windows=True)
for cli in ("pi","omp"):
    run("extensions-missing-"+cli,"_extensoes",cli)
    folder="."+cli+"/agent/extensions/"
    names=ns["_EXTENSOES_POR_CLI"][cli]
    run("extensions-personal-"+cli,"_extensoes",cli,{folder+name+".ts":"fixture" for name in names},[folder+"lib"])
    run("extensions-other-"+cli,"_extensoes",cli,{"other/source.ts":"fixture"},links={folder+names[0]+".ts":"$ROOT/other/source.ts"})
for name,content in [("missing",None),("broken","{"),("empty",'{}')]:
    run("claude-hooks-"+name,"_hooks_claude",files={} if content is None else {".claude/settings.json":content})
commands="state_hook.py askq_capture.py preview_hook.py subagent_hook.py pair-context nav_hook.py"
for name,binary in [("missing",False),("present",True)]:
    files={".claude/settings.json":json.dumps({"hooks":{"SessionStart":[{"hooks":[{"command":"\"$ROOT/bin/server\" "+commands}]}]}})}
    if binary:files["bin/server"]="fixture"
    run("claude-hooks-binary-"+name,"_hooks_claude",files=files)
Path(__file__).with_name("checks.json").write_text(json.dumps({
    "source_commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
    "command":"python backend/tests/fixtures/contract/harness-health/generate_checks.py",
    "cases":cases,
},ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} checagens capturadas")
