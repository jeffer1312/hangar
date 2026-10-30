"""Mede a trava original no Windows, usando somente uma pasta temporária própria."""
import argparse
import ast
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--source', required=True)
parser.add_argument('--child')
arguments = parser.parse_args()
if os.name != 'nt':
    raise SystemExit('A captura exige Windows real.')
source = Path(arguments.source).read_text(encoding='utf-8')
function = next(node for node in ast.parse(source).body if isinstance(node, ast.FunctionDef) and node.name == '_trava')
namespace = dict(Path=Path, os=os, contextmanager=contextmanager)
exec(compile(ast.Module(body=[function], type_ignores=[]), 'codex_instrucoes.py', 'exec'), namespace)
if arguments.child:
    try:
        with namespace['_trava'](Path(arguments.child)):
            print(json.dumps({'acquired': True}))
    except OSError as error:
        print(json.dumps({'error': type(error).__name__, 'errno': error.errno, 'winerror': getattr(error, 'winerror', None)}))
else:
    with tempfile.TemporaryDirectory(prefix='hangar-instructions-lock-') as raw:
        home = Path(raw)
        with namespace['_trava'](home):
            started = time.monotonic()
            child = subprocess.run([sys.executable, __file__, '--source', arguments.source, '--child', raw], capture_output=True, text=True, timeout=20, check=True)
            elapsed = time.monotonic() - started
        try:
            locked_bytes = (home / '.hangar-instrucoes.lock').read_bytes().hex()
        except OSError as error:
            locked_bytes = dict(error=type(error).__name__, errno=error.errno)
        try:
            with namespace['_trava'](home):
                reacquired = True
        except OSError as error:
            reacquired = dict(error=type(error).__name__, errno=error.errno)
    result = dict(source_commit='8a3ce81c1',
                  command='python capture_instructions_lock.py --source original-codex-instrucoes.py',
                  platform=sys.platform, python=sys.version.split()[0],
                  elapsed_seconds=elapsed, contender=json.loads(child.stdout),
                  locked_bytes=locked_bytes, reacquired=reacquired)
    Path(__file__).with_name('instructions-lock-windows.json').write_text(json.dumps(result, ensure_ascii=True, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result))
