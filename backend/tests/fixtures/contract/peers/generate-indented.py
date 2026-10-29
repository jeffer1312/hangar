"""Captura bytes do escritor indentado e recusas Unicode, sem estado durável."""
import argparse,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);a=p.parse_args()
inputs=['{"text":"açúcar"}','{"value":NaN}','{"value":Infinity}','{"value":-Infinity}','{"value":123456789012345678901234567890}','{"value":"\\ud800"}']
for code in [0x10f800,0x10ffff,0x1f600]:inputs.extend([json.dumps({'value':chr(code)}),json.dumps({chr(code):'value'})])
rows=[]
for raw in inputs:
 for indent in [0,2]:
  try:result={'hex':json.dumps(json.loads(raw),ensure_ascii=False,indent=indent).encode('utf-8').hex()}
  except UnicodeEncodeError:result={'error':'UnicodeEncodeError'}
  rows.append(dict(input=raw,indent=indent,result=result))
a.output.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),command='uv run --project backend python backend/tests/fixtures/contract/peers/generate-indented.py --output backend/tests/fixtures/contract/peers/indented.json',cases=rows),indent=2)+'\n')
print(len(rows),'casos capturados')
