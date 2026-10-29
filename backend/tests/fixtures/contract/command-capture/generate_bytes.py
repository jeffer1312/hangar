"""Captura subprocess.run binário usado pelo preparo ECC, sem instalar programas."""
import json
from pathlib import Path
import subprocess
import tempfile
ROOT=Path(__file__).resolve().parents[5]
cases=[]
for name,script,budget in [
 ('binary',"printf '\\377\\000a\\r\\n'; printf '\\376b\\r' >&2",2),
 ('failure','printf stdout; printf stderr >&2; exit 7',2),
 ('cwd','printf %s "$PWD"',2),
 ('partial','printf before; printf error >&2; /bin/sleep 2',0.15),
 ('timeout-empty','/bin/sleep 2',0.15),
]:
    with tempfile.TemporaryDirectory() as raw:
        cwd=Path(raw)/'work';cwd.mkdir()
        try:
            result=subprocess.run(['/bin/sh','-c',script],cwd=cwd,stdin=subprocess.DEVNULL,capture_output=True,timeout=budget)
            expected=dict(success=result.returncode==0,stdout=result.stdout,stderr=result.stderr)
        except subprocess.TimeoutExpired as error:
            expected=dict(error='timeout',stdout=error.stdout or b'',stderr=error.stderr or b'')
        for key in ('stdout','stderr'):expected[key+'_hex']=expected.pop(key).replace(raw.encode(),b'$ROOT').hex()
        cases.append(dict(name=name,script=script,budget_ms=int(budget*1000),expected=expected))
Path(__file__).with_name('bytes.json').write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
 command='cd backend && uv run python tests/fixtures/contract/command-capture/generate_bytes.py',cases=cases),indent=2)+'\n')
print(len(cases))
