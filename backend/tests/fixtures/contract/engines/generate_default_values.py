"""Captura igualdade Python ao preservar valores de modelo editados à mão."""
import json
import subprocess
import tempfile
from pathlib import Path
from app import default_model
cases = []
for old, new in [(True,1),(False,0),(1,1.0),(2**60,float(2**60)),(2**60+1,float(2**60)),([True],[1]),({'x':False},{'x':0})]:
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder)
        path = home / 'settings.json'
        initial = json.dumps({'model':old,'other':1})
        path.write_text(initial,encoding='utf-8')
        before = default_model.snapshot(home)
        after_cli = {'model':new,'other':2}
        path.write_text(json.dumps(after_cli),encoding='utf-8')
        changed = default_model.restore(home,before)
        cases.append({'initial':initial,'after_cli':after_cli,'changed':changed,'after_restore':json.loads(path.read_text())})
Path(__file__).with_name('default-values.json').write_text(json.dumps({
    'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'command':'PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_default_values.py',
    'cases':cases},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
