"""Confere shutil.which real em executáveis temporários, sem executar o shell."""
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from unittest.mock import patch

assert os.name=="posix", "A captura de permissões usa POSIX"
ROOT=Path(__file__).resolve().parents[5]
source=ast.parse(subprocess.check_output(["git", "show", "9e5e5d4e3fc280f43e17d10da99db662fafe0877:backend/app/harness_saude.py"], cwd=ROOT, text=True))
ns=dict(Path=Path,os=os,shutil=shutil,_E_WINDOWS=False)
exec(compile(ast.Module(body=[n for n in source.body if isinstance(n,ast.FunctionDef) and n.name in {"_item","_wrapper","_perfis_powershell"}],type_ignores=[]),"original-wrapper-executable","exec"),ns)
cases=[]
for name,modes in [("missing",[]),("not-executable",[0o600]),("executable",[0o700]),("later-executable",[0o600,0o700])]:
    with tempfile.TemporaryDirectory() as raw:
        home=Path(raw);(home/".bashrc").write_text("")
        dirs=[]
        for index,mode in enumerate(modes):
            directory=home/f"bin-{index}";directory.mkdir();program=directory/"bash";program.write_text("fixture");program.chmod(mode);dirs.append(str(directory))
        with patch.dict(os.environ,{"HOME":raw,"USERPROFILE":raw,"PATH":os.pathsep.join(dirs)},clear=True):
            result=ns["_wrapper"]("claude")
            program=shutil.which("bash")
        cases.append(dict(name=name,modes=modes,expected=result,program=program.replace(raw,"$ROOT") if program else None))
Path(__file__).with_name("wrapper-executable.json").write_text(json.dumps(dict(
    source_commit='9e5e5d4e3fc280f43e17d10da99db662fafe0877',
    platform="posix",command="python backend/tests/fixtures/contract/harness-health/generate_wrapper_executable.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} condições de executável capturadas")
