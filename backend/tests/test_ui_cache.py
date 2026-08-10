"""`index.html` tem de revalidar; os assets com hash no nome, nao.

Sem `cache-control` no HTML o navegador aplica frescor heuristico e serve a pagina guardada sem
perguntar — a pagina velha continua pedindo o bundle velho e o build novo nunca aparece na tela.
Foi medido em 10/08/2026 (ver a docstring de `_UIStatic` em app/api.py).
"""
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import _UIStatic


def _cli(tmp_path) -> TestClient:
    (tmp_path / "index.html").write_text(
        '<!doctype html><link rel=stylesheet href="/assets/index-abc123.css">', encoding="utf-8"
    )
    (tmp_path / "assets").mkdir()
    (tmp_path / "assets" / "index-abc123.css").write_text("body{}", encoding="utf-8")
    app = FastAPI()
    app.mount("/", _UIStatic(directory=tmp_path, html=True), name="ui")
    return TestClient(app)


def test_index_revalida(tmp_path):
    r = _cli(tmp_path).get("/")
    assert r.status_code == 200
    assert r.headers["cache-control"] == "no-cache"
    # `no-cache` e nao `no-store`: o navegador segue guardando o arquivo, so volta a perguntar.
    assert "no-store" not in r.headers["cache-control"]


def test_asset_com_hash_nao_ganha_no_cache(tmp_path):
    r = _cli(tmp_path).get("/assets/index-abc123.css")
    assert r.status_code == 200
    assert "cache-control" not in r.headers


def test_index_ainda_responde_304(tmp_path):
    """O ETag continua valendo — revalidar custa uma resposta vazia, nao o HTML de novo."""
    cli = _cli(tmp_path)
    etag = cli.get("/").headers["etag"]
    r = cli.get("/", headers={"if-none-match": etag})
    assert r.status_code == 304


def test_304_tambem_carrega_a_diretiva(tmp_path):
    """O 304 do starlette e um `NotModifiedResponse`, que NAO copia o `content-type`.

    Decidir pelo header da resposta pronta deixava justamente a revalidacao sem diretiva nenhuma —
    navegador nao regride (mescla com o que ja guardou), proxy na frente pode regredir.
    """
    cli = _cli(tmp_path)
    etag = cli.get("/").headers["etag"]
    r = cli.get("/", headers={"if-none-match": etag})
    assert r.status_code == 304
    assert r.headers.get("cache-control") == "no-cache"
