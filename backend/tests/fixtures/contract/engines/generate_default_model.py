"""Captura proteção do modelo global, incluindo as duas escritas tardias do CLI."""
import itertools
import json
import subprocess
import tempfile
from pathlib import Path
from unittest.mock import patch
from app import default_model
cases = []
for initial in [None, "", "{ inválido", "[]", '{}', '{"model":null}', '{"model":"sonnet","other":1}']:
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder)
        path = home / 'settings.json'
        if initial is not None:
            path.write_text(initial, encoding='utf-8')
        before = default_model.snapshot(home)
        path.write_text('{"model":"engine-id","other":2}', encoding='utf-8')
        changed = default_model.restore(home, before)
        cases.append({'initial':initial, 'after_cli':{'model':'engine-id','other':2},
                      'changed':changed, 'after_restore':json.loads(path.read_text())})
late = []
for twice in [False, True]:
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder)
        path = home / 'settings.json'
        path.write_text('{"model":"sonnet","other":1}', encoding='utf-8')
        before = default_model.snapshot(home)
        sleeps = []
        def sleep(seconds):
            sleeps.append(seconds)
            if len(sleeps) == 2 or (twice and seconds == default_model._RECONFERE_APOS):
                path.write_text('{"model":"engine-id","other":2}', encoding='utf-8')
        with patch.object(default_model.time,'sleep',side_effect=sleep), \
             patch.object(default_model.time,'monotonic',side_effect=itertools.count(step=0.1)):
            changed = default_model.restore_quando_aterrissar(home,before)
        late.append({'twice':twice,'changed':changed,'sleeps':sleeps,'after_restore':json.loads(path.read_text())})
Path(__file__).with_name('default-model.json').write_text(json.dumps({
    'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'command':'PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_default_model.py',
    'cases':cases,'late':late},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
