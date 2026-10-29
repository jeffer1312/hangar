"""Captura os metadados públicos do ambiente sem ler configuração pessoal."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from app.config import Settings, variaveis_env

cases = []
inputs = [{}, {'CP_PORT': '9876', 'CP_FRONT_PORT': '', 'CP_AUTH_TOKEN': 'fixture-secret',
                 'CP_VAPID_PRIVATE': 'fixture-private', 'CP_SYNC_BOOTSTRAP': 'fixture-bootstrap',
                 'CP_RELOAD': 'true', 'CP_OMP_PLUGIN_SYNC_INTERVAL': '2.5', 'CP_RUST_SERVER': 'false',
                 'CP_CODEX_SYNC_ENABLED': 'FALSE', 'CP_TERMINAL': 'fixture-terminal', 'CP_SERVER_ID': 'fixture'}]
inputs.extend({'CP_OMP_PLUGIN_SYNC_INTERVAL': value} for value in
              ['1_0', ' 1.5 ', '1e2', '+2', '01', '.5', '1.', '2_._0', '_1', '1_', '__1__', '1e_2', '1_e2', '1__2', '+_2', '-_2', '1e+_2', '1_e_2', '1_._0', '١٢', 'NaN', 'Infinity', '0', '-1', ''])
inputs.extend({'CP_PORT': value} for value in ['+123', '00123', '1_234', '123.0', '123.5'])
inputs.extend({'CP_UPDATE_BRANCH': value} for value in [' feature/topic ', '-option', 'a..b', 'a b', '/topic', ''])
for env in inputs:
    with tempfile.TemporaryDirectory() as folder, patch.dict(os.environ, {'HOME': folder, 'USERPROFILE': folder, **env}, clear=True):
        try:
            settings = Settings(_env_file=None, projects_dir=Path(folder) / '.claude/projects', sync_data=Path(folder) / '.hangar/sync-vault.json')
        except ValueError:
            cases.append({'env': env, 'invalid': True})
            continue
        expected = variaveis_env(settings)
        expected = json.loads(json.dumps(expected).replace(folder, '⟦HOME⟧'))
        cases.append({'env': env, 'metadata': expected})
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_metadata.py',
          'injections': ['ambiente vazio', 'projects_dir e sync_data na casa isolada'], 'cases': cases}
Path(__file__).with_name('metadata.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
