"""Captura instalação de plugins e serialização numérica no Python original."""
import asyncio
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

BACKEND = Path(__file__).resolve().parents[4]
sys.path[:0] = [str(BACKEND), str(BACKEND / 'tests')]
from pytest import MonkeyPatch
from app import config_sync
from config_sync_machines import make_machine, use_machine


async def main():
    cases = []
    for mode in ['success', 'failed', 'installed', 'sources']:
        with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as patch:
            base = Path(folder)
            roots = make_machine(base, 'target', full=False)
            use_machine(patch, roots)
            patch.setattr(config_sync.shutil, 'which', lambda name: '/fixture/claude' if name == 'claude' else None)
            sources = {'repo': {'source': 'github', 'repo': 'group/project'}}
            enabled = {'active@repo': True, 'disabled@repo': False}
            if mode == 'sources':
                sources = {kind: {'source': kind, key: value} for kind, key, value in [
                    ('git', 'url', 'https://example.invalid/repo.git'),
                    ('url', 'url', 'https://example.invalid/catalog.json'),
                    ('github', 'repo', 'group/project'),
                    ('directory', 'path', '⟦HOME⟧/market'),
                    ('file', 'path', '⟦HOME⟧/market.json'),
                    ('unknown', 'url', 'ignored'),
                    ('empty', 'url', ''),
                ]}
            plugins = Path(roots.claude) / 'plugins'
            plugins.mkdir()
            known = {'repo': {}} if mode == 'installed' else {}
            installed = {'active@repo': [{}]} if mode == 'installed' else {}
            (plugins / 'known_marketplaces.json').write_text(json.dumps(known))
            (plugins / 'installed_plugins.json').write_text(json.dumps({'plugins': installed}))
            data = {'enabledPlugins': enabled, 'extraKnownMarketplaces': {}, 'sources': sources}
            bundle = config_sync.Bundle(items={'claude_plugins': data})
            calls, progress = [], []
            def runner(args):
                calls.append(args)
                return (False, 'fatal: repo not found') if mode == 'failed' else (True, '')
            async def after(*args):
                pass
            report = await config_sync.apply_bundle(bundle, ['claude_plugins'], roots,
                runner=runner, after=after, progress=progress.append)
            case = {'mode': mode, 'data': data, 'known': known, 'installed': installed,
                    'calls': calls, 'progress': progress, 'report': report,
                    'settings': json.loads((Path(roots.claude) / 'settings.json').read_text())}
            cases.append(json.loads(json.dumps(case, ensure_ascii=False).replace(roots.home, '⟦HOME⟧')))
    values = [1.0, -0.0, 1e-7, 1e20, 1e21, 10**80, {'é': 1e-6, 'a': [1.0, 1e16]}]
    hashes = []
    for value in values:
        raw = json.dumps(value, ensure_ascii=False, sort_keys=True)
        hashes.append({'value': value, 'serialized': raw,
                       'hash': hashlib.sha256(raw.encode()).hexdigest()})
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_plugins.py',
              'injections': ['CLI capturado sem processo', 'pós-aplicação isolada'],
              'cases': cases, 'hashes': hashes}
    Path(__file__).with_name('plugins.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    asyncio.run(main())
