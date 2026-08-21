"""Fixtures compartilhadas da suíte.

`models_cache_em_tmp` redireciona o espelho em DISCO do cache de modelos (o
`.claude-pocket-models.json` que `app.api` grava DENTRO do config dir) pra um tmp_path — sem isso,
os testes que exercitam as rotas de model-options escrevem no `~/.claude` REAL de quem roda a
suíte, e os testes seguintes leem esse cache no lugar do mock (aconteceu na primeira rodada: a
lista real da conta vazou pra dentro de três testes).

NÃO é autouse global de propósito: a primeira versão autouse-pra-tudo derrubou 13 testes de
test_tmux/test_auth_backoff por interação de fixtures na suíte cheia. Cada arquivo que toca nas
rotas liga o isolamento com um autouse LOCAL de uma linha (ver test_api.py e
test_model_options_api.py).
"""

import sys

import pytest


@pytest.fixture
def models_cache_em_tmp(tmp_path, monkeypatch):
    api = sys.modules.get("app.api")
    if api is None:
        yield
        return

    def _path_de_teste(chave: str):
        return tmp_path / f"models-{chave.replace(chr(47), chr(95))}.json"

    monkeypatch.setattr(api, "_models_cache_path", _path_de_teste)
    api._claude_models_cache.clear()
    yield
    api._claude_models_cache.clear()
