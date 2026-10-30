"""Captura a trava comum entre a administração de sync e as preferências."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from unittest.mock import patch

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from pytest import MonkeyPatch
from app import runtime_config as rc, sync
from app.config import Settings

with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(
    os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True
):
    base = Path(folder)
    path = base / 'runtime-config.json'
    path.write_text('{"sync": true}')
    mp.setattr(rc, '_backend_config_base', lambda: base)
    mp.setattr(rc, '_cache', None)
    mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
    mp.setattr(sync, '_load_setup_vault', lambda: None)
    mp.setattr(sync.diag, 'registrar', lambda *_args, **_kwargs: None)
    started = threading.Event()
    finished = threading.Event()
    result = {}

    def request():
        started.set()
        try:
            result['response'] = sync.disable()
        except Exception as exc:
            result['error'] = type(exc).__name__
        finally:
            finished.set()

    with rc._LOCK:
        worker = threading.Thread(target=request)
        worker.start()
        assert started.wait(5)
        result['blocked_by_runtime_writer'] = not finished.wait(0.1)
        path.write_text('{"sync": true, "editor": "vim"}')
    worker.join(5)
    assert not worker.is_alive()
    result['file'] = json.loads(path.read_text())
    assert result['blocked_by_runtime_writer'] and result['file'] == {'sync': False, 'editor': 'vim'}

Path(__file__).with_name('sync-lock.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_sync_lock.py',
    'injections': ['cofre vazio isolado; diário substituído para não escrever fora da prova'],
    'result': result,
}, ensure_ascii=True, indent=2) + '\n')
