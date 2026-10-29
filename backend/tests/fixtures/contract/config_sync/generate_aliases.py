"""Congela a leitura e as escritas de apelido executando o módulo original."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from pytest import MonkeyPatch
from app import apelidos


def main():
    cases = []
    for original in [None, '{}', '[]', '{', '{"a": "  Original  ", "vazio": " ", "numero": 4}']:
        for value in [None, '', '  ', '  Nome  ', '𝄞á' * 50]:
            with tempfile.TemporaryDirectory() as folder, MonkeyPatch.context() as patch:
                patch.setenv('HOME', folder)
                path = Path(folder) / '.claude' / '.hangar-apelidos.json'
                path.parent.mkdir()
                if original is not None:
                    path.write_text(original, encoding='utf-8')
                before = apelidos.ler()
                result = apelidos.definir('a', value)
                cases.append({'initial': original, 'value': value, 'before': before,
                              'result': result, 'raw': path.read_text(encoding='utf-8')})
    data = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_aliases.py',
            'cases': cases}
    Path(__file__).with_name('aliases.json').write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
