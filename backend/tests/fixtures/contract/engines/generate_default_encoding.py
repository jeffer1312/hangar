"""Captura o erro de decodificação antes de qualquer troca de modelo."""
import json
import subprocess
import tempfile
from pathlib import Path
from app import default_model
cases = []
for operation in ['snapshot', 'restore']:
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder)
        path = home / 'settings.json'
        path.write_text('{"model":"sonnet"}', encoding='utf-8')
        before = default_model.snapshot(home)
        path.write_bytes(b'\xff')
        try:
            value = default_model.snapshot(home) if operation == 'snapshot' else default_model.restore(home, before)
        except Exception as error:
            cases.append({'operation':operation,'raw_hex':'ff','error':type(error).__name__,'after_hex':path.read_bytes().hex()})
        else:
            raise AssertionError(f'{operation}: esperava falha, veio {value!r}')
Path(__file__).with_name('default-encoding.json').write_text(json.dumps({
    'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'command':'PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_default_encoding.py',
    'cases':cases},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
