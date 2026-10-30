"""Captura as opções vivas do tmux com respostas de subprocesso controladas."""
import ast
import json
import os
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[5]
source=ast.parse(subprocess.check_output(["git", "show", "9e5e5d4e3fc280f43e17d10da99db662fafe0877:backend/app/harness_saude.py"], cwd=ROOT, text=True))
ns=dict(Path=Path,os=os)
exec(compile(ast.Module(body=[n for n in source.body if isinstance(n,ast.FunctionDef) and n.name in {"_item","_conf_psmux","_card_tmux"}],type_ignores=[]),"original-tmux-health","exec"),ns)
cases=[]
for name,spec in [
    ("missing",{"version":None}),("version-error",{"version":""}),
    ("missing-options",{}),
    ("correct",{"config":"# >>> hangar >>>","terminal":"xterm-256color","environment":"COLORTERM=truecolor\nCLAUDE_CODE_TMUX_TRUECOLOR=1","title":"#S","mouse":"on","persist":True}),
    ("screen-terminal",{"terminal":"screen-256color"}),
    ("tmux-terminal",{"terminal":"tmux-256color"}),
    ("one-truecolor",{"environment":"COLORTERM=truecolor"}),
    ("wrong-title",{"title":"#H"}),
    ("windows",{"windows":True,"config":"# >>> hangar windows-tmux >>>","terminal":"xterm-256color"}),
    ("windows-old-marker",{"windows":True,"config":"# >>> hangar >>>"}),
    ("bom",{"config":"\ufeff# >>> hangar >>>"}),
]:
    calls=[]
    with tempfile.TemporaryDirectory() as raw:
        home=Path(raw)
        if "config" in spec:(home/".tmux.conf").write_text(spec["config"])
        if spec.get("persist"):(home/".tmux/plugins/tmux-resurrect").mkdir(parents=True)
        def tmux(args):
            calls.append(args)
            return spec.get({"default-terminal":"terminal","show-environment":"environment","set-titles-string":"title","mouse":"mouse"}.get(args[-1] if args[0]=="show" else args[0],""))
        ns.update(_E_WINDOWS=spec.get("windows",False),_versao=lambda _:spec.get("version","tmux-fixture"),_tmux=tmux)
        with patch.dict(os.environ,{"HOME":raw,"USERPROFILE":raw},clear=True):result=ns["_card_tmux"]()
    cases.append(dict(name=name,input=spec,expected=dict(card=result,calls=calls)))
Path(__file__).with_name("tmux.json").write_text(json.dumps(dict(
    source_commit='9e5e5d4e3fc280f43e17d10da99db662fafe0877',
    command="python backend/tests/fixtures/contract/harness-health/generate_tmux.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} cenários de tmux capturados")
