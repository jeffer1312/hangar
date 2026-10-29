"""Executa exportação e aplicação originais, preservando entradas e efeitos em disco."""
import asyncio
import base64
import dataclasses
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


def tree(base):
    result = {}
    for path in sorted(base.rglob('*')):
        name = path.relative_to(base).as_posix()
        if path.is_symlink():
            result[name] = {'link': str(path.readlink()).replace(str(base), '⟦FIXTURE⟧')}
        elif path.is_file():
            raw = path.read_bytes()
            try:
                content = {'text': raw.decode('utf-8').replace(str(base), '⟦FIXTURE⟧')}
            except UnicodeDecodeError:
                content = {'base64': base64.b64encode(raw).decode()}
            result[name] = {**content, 'mode': path.stat().st_mode & 0o777}
    return result


async def main():
    cases = []
    for selected in [[item] for item in config_sync.ITEMS] + [list(config_sync.ITEMS)]:
        with tempfile.TemporaryDirectory(prefix='config-sync-bundle-') as folder, MonkeyPatch.context() as patch:
            base = Path(folder)
            source = make_machine(base, 'source')
            target = make_machine(base, 'target', full=False)
            use_machine(patch, source)
            patch.setattr(config_sync.shutil, 'which', lambda name: None)
            initial = tree(base)
            progress = []
            bundle = config_sync.export_bundle(source, selected, progress=progress.append)
            packed = config_sync.pack(bundle)
            restored = config_sync.unpack(packed)
            use_machine(patch, target)
            patch.setattr(config_sync.time, 'strftime', lambda *args: '20000101-000000')
            class FixedUuid:
                hex = '0123456789abcdef0123456789abcdef'
            patch.setattr(config_sync.uuid, 'uuid4', lambda: FixedUuid())
            async def after(*args, **kwargs):
                return None
            report = await config_sync.apply_bundle(restored, selected, target, after=after)
            record = {
                'items': selected, 'source': dataclasses.asdict(source), 'target': dataclasses.asdict(target),
                'initial': initial, 'bundle': {'items': bundle.items, 'warnings': bundle.warnings,
                    'files': {name: {'data': base64.b64encode(blob.data).decode(), 'mode': blob.mode}
                              for name, blob in bundle.files.items()}},
                'report': report, 'final': tree(base), 'progress': progress,
            }
            cases.append(json.loads(json.dumps(record, ensure_ascii=False).replace(str(base), '⟦FIXTURE⟧')))
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_bundle.py',
              'injections': ['árvore temporária', 'PATH sem CLIs', 'pós-aplicação isolada', 'relógio e UUID fixos'],
              'cases': cases}
    Path(__file__).with_name('bundles.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    asyncio.run(main())
