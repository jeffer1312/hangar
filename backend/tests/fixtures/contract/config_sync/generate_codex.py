"""Captura decisões de edição Codex; o escritor nativo só registra os pedidos recebidos."""
import asyncio
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


async def main():
    cases = []
    for mode in ['merge', 'same', 'missing', 'programs', 'invalid']:
        with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as patch:
            roots = make_machine(Path(folder), 'target', full=False)
            use_machine(patch, roots)
            current = 'model = "antigo"\n[mcp_servers.hangar]\nurl = "http://127.0.0.1:9999/mcp/"\n'
            incoming = {'model': 'novo', 'mcp_servers': {'docs': {'url': 'https://example.invalid/mcp'}}}
            if mode == 'same':
                current = 'model = "novo"\n'
                incoming = {'model': 'novo'}
            elif mode == 'programs':
                incoming = {'mcp_servers': {'docs': {'command': '⟦HOME⟧/.local/share/fnm/v24/bin/node'}}}
            elif mode == 'invalid':
                current = 'model = ['
            path = Path(roots.codex) / 'config.toml'
            path.write_text(current)
            calls = []
            class Native:
                def __init__(self, home, codex, binary):
                    self.codex = codex
                async def __aenter__(self):
                    return self
                async def __aexit__(self, *args):
                    pass
                async def request(self, method, params):
                    calls.append({'method': method, 'params': params})
                    (self.codex / 'config.toml').write_text('model = "nativo"\n')
            patch.setattr(config_sync, '_NATIVE', Native)
            programs = {'node': '/fixture/node'}
            if mode != 'missing':
                programs['codex'] = '/fixture/codex'
            patch.setattr(config_sync.shutil, 'which', programs.get)
            bundle = config_sync.Bundle(items={'codex': {'config': incoming}})
            async def after(*args):
                pass
            patch.setattr(config_sync.time, 'strftime', lambda *args: '20000101-000000')
            class FixedUuid:
                hex = '0123456789abcdef0123456789abcdef'
            patch.setattr(config_sync.uuid, 'uuid4', lambda: FixedUuid())
            report = await config_sync.apply_bundle(bundle, ['codex'], roots, after=after)
            result = {'mode': mode, 'roots': dataclasses.asdict(roots), 'current': current,
                      'incoming': incoming, 'programs': programs, 'calls': calls,
                      'report': report, 'final': path.read_text()}
            cases.append(json.loads(json.dumps(result, ensure_ascii=False).replace(roots.home, '⟦HOME⟧')))
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_codex.py',
              'injections': ['escritor TOML capturado', 'pós-aplicação isolada', 'relógio e UUID fixos'],
              'cases': cases}
    Path(__file__).with_name('codex.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    asyncio.run(main())
