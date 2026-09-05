"""Cache do anexo citado na conversa (GET /api/sessions/{name}/file).

A miniatura de 96px carrega o arquivo ORIGINAL, entao sem cache toda repintura da lista rebaixava o
PNG inteiro. Aqui vale o par: `cache-control` pra nao perguntar por 60s, e 304 pra quando perguntar.
"""
import pytest
from fastapi.testclient import TestClient

from app import api, transcript
from app.api import app
from app.config import settings
from app.models import SessionInfo


@pytest.fixture
def cliente():
    anterior = settings.auth_token
    settings.auth_token = "secret"
    yield TestClient(app)
    settings.auth_token = anterior


@pytest.fixture
def sessao(tmp_path, monkeypatch):
    """Sessao de mentira apontando pro tmp_path, com a trava do transcript satisfeita."""
    jsonl = tmp_path / "conversa.jsonl"
    jsonl.write_text("{}\n", encoding="utf-8")
    info = SessionInfo(name="s1", cwd=str(tmp_path), jsonl=str(jsonl), tracked=True)
    monkeypatch.setattr(api, "_cached_info_sync", lambda nome: info if nome == "s1" else None)
    return tmp_path


def _pegar(cliente, arquivo, **kw):
    return cliente.get("/api/sessions/s1/file", params={"path": arquivo},
                       headers={"Authorization": "Bearer secret", **kw.pop("headers", {})}, **kw)


def test_anexo_volta_com_cache_e_etag(cliente, sessao, monkeypatch):
    monkeypatch.setattr(transcript, "path_in_transcript", lambda *a: True)
    (sessao / "foto.png").write_bytes(b"\x89PNG-um")

    r = _pegar(cliente, "foto.png")
    assert r.status_code == 200
    assert r.content == b"\x89PNG-um"
    assert r.headers["cache-control"] == "max-age=60"
    assert r.headers["etag"]


def test_mesmo_etag_volta_304_sem_corpo(cliente, sessao, monkeypatch):
    monkeypatch.setattr(transcript, "path_in_transcript", lambda *a: True)
    (sessao / "foto.png").write_bytes(b"\x89PNG-um")

    etag = _pegar(cliente, "foto.png").headers["etag"]
    r = _pegar(cliente, "foto.png", headers={"If-None-Match": etag})
    assert r.status_code == 304
    assert r.content == b""
    assert r.headers["cache-control"] == "max-age=60"


def test_arquivo_reescrito_invalida_o_etag(cliente, sessao, monkeypatch):
    """O ETag carrega mtime+tamanho: reescrever no MESMO caminho tem que voltar a mandar o corpo,
    senao um mock regenerado ficaria preso na versao velha ate o navegador desistir sozinho."""
    monkeypatch.setattr(transcript, "path_in_transcript", lambda *a: True)
    alvo = sessao / "mock.html"
    alvo.write_text("<p>um</p>", encoding="utf-8")
    etag_velho = _pegar(cliente, "mock.html").headers["etag"]

    import os
    alvo.write_text("<p>dois</p>", encoding="utf-8")
    os.utime(alvo, (0, 0))  # mtime diferente sem depender do relogio do teste

    r = _pegar(cliente, "mock.html", headers={"If-None-Match": etag_velho})
    assert r.status_code == 200
    assert r.content == b"<p>dois</p>"


def test_304_nao_fura_a_trava_do_transcript(cliente, sessao, monkeypatch):
    """304 e resposta SOBRE um arquivo: quem nao pode ve-lo tambem nao pode saber que ele mudou.
    Sem esta ordem, um If-None-Match viraria oraculo de existencia/mtime de qualquer caminho."""
    monkeypatch.setattr(transcript, "path_in_transcript", lambda *a: False)
    (sessao / "segredo.png").write_bytes(b"x")

    r = _pegar(cliente, "segredo.png", headers={"If-None-Match": '"qualquer"'})
    assert r.status_code == 403
