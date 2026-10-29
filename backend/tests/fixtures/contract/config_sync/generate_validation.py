"""Executa o modelo FastAPI original para congelar a validação da tradução."""
import json
from pathlib import Path
import subprocess
import sys

BACKEND = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(BACKEND))
from fastapi import FastAPI
from fastapi.testclient import TestClient
from app.config_sync_api import TranslateBody


def main():
    app = FastAPI()
    @app.post('/translate')
    def translate(body: TranslateBody):
        return body.model_dump()
    bodies = ['', 'null', '[]', '"texto"', '{}', '{', '{"lang":', '{"lang":"pt",}',
              *[json.dumps({'lang': lang, 'texts': []}, ensure_ascii=False) for lang in [None, False, 1, 'PT', 'xx']],
              *[json.dumps({'lang': 'pt', 'texts': texts}, ensure_ascii=False) for texts in [None, False, 1, {}, 'texto', [False], [None], [1], [{}], ['x' * 2001]]],
              '{"lang":"pt"}', '{"texts":[]}', '{"lang":"pt","texts":[],"extra":true}',
              '{"lang":false,"texts":[null,1],"extra":true}']
    cases = []
    with TestClient(app) as client:
        for raw in bodies:
            response = client.post('/translate', content=raw, headers={'Content-Type': 'application/json'})
            cases.append({'raw': raw, 'status': response.status_code, 'body': response.json()})
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate_validation.py',
              'cases': cases}
    Path(__file__).with_name('validation.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
