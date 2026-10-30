"""Captura a vida do trabalho de progresso depois que o consumidor fecha a resposta."""
import asyncio
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from app.config_sync_api import _ndjson

async def capture():
    release, done = asyncio.Event(), asyncio.Event()
    events = []
    async def work(progress):
        events.append('started')
        progress({'phase': 'apply'})
        await release.wait()
        events.append('finished')
        done.set()
        return {'ok': True}
    response = _ndjson(work)
    first = await anext(response.body_iterator)
    await response.body_iterator.aclose()
    events.append('closed')
    release.set()
    await asyncio.wait_for(done.wait(), 2)
    return {'first': json.loads(first), 'events': events}

Path(__file__).with_name('apply-disconnect.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_apply_disconnect.py',
    'injections': ['trabalho retido por evento; consumidor real do iterator de StreamingResponse'],
    'result': asyncio.run(capture()),
}, ensure_ascii=True, indent=2) + '\n')
