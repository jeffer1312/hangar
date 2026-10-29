"""Captura diagnóstico e posição de whitespace e finais de contêiner incompletos."""
import json
from pathlib import Path
import subprocess
ROOT=Path(__file__).resolve().parents[5]
cases=[]
for prefix in ['', '[', '{', '{"a":', '[1,', '{"a":1,', '[1', '{"a":1']:
    for whitespace in [' ', '\t', '\n', '\r\n', ' \t\r\n ', '\u00a0', '\u001c']:
        raw=prefix+whitespace
        for encoding in ['utf-8','utf-16','utf-32']:
            data=raw.encode(encoding)
            try:result={'value_json':json.dumps(json.loads(data),ensure_ascii=True)}
            except json.JSONDecodeError as error:result={'position':error.pos,'message':error.msg,'status':422}
            cases.append(dict(raw=raw,encoding=encoding,hex=data.hex(),**result))
Path(__file__).with_name('json-whitespace.json').write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
 command='cd backend && uv run python tests/fixtures/contract/config_sync/generate_json_whitespace.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
