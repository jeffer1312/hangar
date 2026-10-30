"""Captura ordem, seleção e avisos do pós-aplicar sem executar instaladores reais."""
import asyncio
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from pytest import MonkeyPatch
from app import config_sync as cs

cases = []
scenarios = [([], False, ''), (['hangar_prefs'], True, '')]
for items in [['claude_skills'], ['claude_plugins'], ['claude_hooks'], ['codex'],
              ['claude_skills', 'claude_hooks', 'claude_plugins', 'codex']]:
    for enabled in [False, True]:
        scenarios.append((items, enabled, ''))
for failure in ['bridge', 'bridge_report', 'codex', *cs._HANGAR_HOOK_INSTALLERS]:
    scenarios.append((['claude_skills', 'claude_hooks', 'claude_plugins', 'codex'], True, failure))

for items, enabled, failure in scenarios:
    with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp:
        calls, events = [], []
        def bridge(home, log):
            calls.append('bridge')
            if failure == 'bridge':
                raise RuntimeError('falha de ponte')
            return {'kimi': {'erro': 1}} if failure == 'bridge_report' else {}
        def installer(name):
            def run():
                calls.append(name)
                if failure == name:
                    raise RuntimeError('falha de hook')
            return run
        async def start(reason, force):
            calls.append(['codex', reason, force])
            if failure == 'codex':
                raise RuntimeError('falha de integração')
        mp.setattr(cs.skill_bridge, 'rebuild', bridge)
        for name in cs._HANGAR_HOOK_INSTALLERS:
            mp.setattr(cs.hook_installer, name, installer(name))
        mp.setattr(cs.runtime_config, 'get', lambda key: enabled)
        mp.setitem(sys.modules, 'app.codex_integracao', SimpleNamespace(SERVICO=SimpleNamespace(iniciar=start)))
        ctx = SimpleNamespace(roots=SimpleNamespace(home=folder), report={}, progress=events.append,
                              bundle=cs.Bundle())
        asyncio.run(cs._after_apply(ctx, items))
        cases.append({'items': items, 'enabled': enabled, 'failure': failure,
                      'calls': calls, 'events': events, 'report': ctx.report})

Path(__file__).with_name('after-apply.json').write_text(json.dumps({
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': 'uv run python tests/fixtures/contract/config_sync/generate_after_apply.py',
    'injections': ['instaladores, ponte e integração substituídos por gravadores de chamadas'],
    'cases': cases,
}, ensure_ascii=True, indent=2) + '\n')
