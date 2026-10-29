"""Captura str/repr de JSON com escalares especiais e árvores profundas."""
import hashlib,json,subprocess,sys
from pathlib import Path
output=Path(sys.argv[1]);rows=[]
values=['null','true','false','1','1.0','1e-7','1e20','NaN','Infinity','-Infinity','"texto"','"a\\\"b"','"a\u0027b"','"\\ud800"','["\\ud800",NaN,Infinity,-Infinity,null,true,false]','{"z":"linha\\nseguinte","a":[1,2.0]}']
for raw in values:
 value=json.loads(raw);rows.append(dict(kind='scalar',raw=raw,output_json=json.dumps(str(value),ensure_ascii=True)))
for depth in [900,1500,10000]:
 for shape,opening,closing in [('array','[',']'),('object','{"x":','}'),('ordered','{"z":0,"x":','}')]:
  raw=opening*depth+'0'+closing*depth
  try:text=str(json.loads(raw)).encode()
  except Exception as error:rows.append(dict(kind='deep',shape=shape,depth=depth,error=type(error).__name__,message=str(error)));continue
  rows.append(dict(kind='deep',shape=shape,depth=depth,bytes=len(text),sha256=hashlib.sha256(text).hexdigest()))
output.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),command='python generate-python-repr.py <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} casos str/repr capturados; {sum("error" in row for row in rows)} recusas')
