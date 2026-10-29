"""Captura a escolha do auxiliar legado com fontes locais isoladas."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from pytest import MonkeyPatch
from app import computer_control as cc, runtime_config as rc
from app.config import Settings

cases = []
sources = [None, {}, {'LLM_PROXY_URL': 'https://example.test/v1/chat/completions?api-version=2', 'LLM_MODEL': 'modelo'},
           {'LLM_PROXY_URL': 'https://example.test/v1', 'LLM_MODEL': 'modelo', 'LLM_PROXY_KEY': 'fixture', 'LLM_EFFORT': 'high'},
           {'LLM_PROXY_URL': 'https://different.test/v1', 'LLM_MODEL': 'outro', 'LLM_PROXY_KEY': 'fixture', 'LLM_EFFORT': 'low'}]
for browser in [{}, {'jev_texto_base_url': 'https://example.test/v1/chat/completions?api-version=2', 'jev_texto_modelo': 'modelo'},
                {'jev_texto_base_url': 'https://example.test/v1', 'jev_texto_modelo': 'modelo', 'jev_texto_api_key': 'fixture'}]:
    for active in sources:
        for parked in [False, True]:
            with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as mp, patch.dict(os.environ, {'HOME': folder, 'USERPROFILE': folder}, clear=True):
                home = Path(folder)
                config = home / '.claude'
                config.mkdir()
                (home / '.hangar').mkdir()
                (config / 'runtime-config.json').write_text(json.dumps(browser))
                mp.setattr(rc, '_backend_config_base', lambda: config)
                mp.setattr(rc, '_cache', None)
                mp.setattr(rc, 'settings', Settings(_env_file=None, scan_roots=''))
                mp.setattr(cc, '_main_file', lambda: home / '.claude.json')
                mp.setattr(cc, '_parked_file', lambda: home / '.hangar/computer-control.json')
                entry = {'command': 'fixture', 'env': active} if active is not None else None
                if entry is not None:
                    path = home / ('.hangar/computer-control.json' if parked else '.claude.json')
                    path.write_text(json.dumps(entry if parked else {'mcpServers': {cc.NAME: entry}}))
                cases.append({'runtime': browser, 'entry': entry, 'parked': parked,
                              'effective': {key: rc.get(key) for key, _ in rc.JEV_AUX}})
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_legacy.py', 'cases': cases}
Path(__file__).with_name('legacy.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
