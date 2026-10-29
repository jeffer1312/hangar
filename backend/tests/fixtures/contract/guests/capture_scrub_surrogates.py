"""Captura models.scrub_surrogates original; leitura e saída JSON preservam casos não finitos."""
import json
import subprocess
import sys
from pathlib import Path
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
from app.models import scrub_surrogates
inputs=[r'""',r'"texto"',r'"a\u00e7\u00e3o"',r'"\ud800"',r'"\udfff"',r'"a\ud800b\udc00c"',
        r'"\ud83d\ude00"',r'"\\ud800"',r'"\ufffd"',r'"\ud800\ud800"',r'"\udc00\ud800"',
        r'{"\ud800":1,"\ufffd":2}',r'{"\ufffd":1,"\ud800":2}',r'{"\ud800":{"x":[1]},"\udc00":[2]}',
        r'["\ud800",{"\udc00":"\udfff"},null,true,42]',r'[NaN,Infinity,-Infinity]',
        r'"\udbff\udfff orq:fixture:surrogate:d800;"',r'"x\u0000y"',
        '['*100+r'"\ud800"'+']'*100,'{"x":'*100+r'"\udc00"'+'}'*100]
rows=[]
for raw in inputs:
    value=json.loads(raw)
    rows.append({'raw':raw,'expected':json.dumps(scrub_surrogates(value))})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_scrub_surrogates.py',
        'isolation':'models.scrub_surrogates original; nenhuma rota ou efeito em disco', 'rows':rows}
(HERE/'scrub-surrogates.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} entradas de normalização capturadas')
