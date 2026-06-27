"""Checks dos fixes: middleware de body-size, cache no rename, prefixo do _open_jsonl."""
from pathlib import Path
from fastapi import FastAPI, Request
from fastapi.testclient import TestClient
from app.api import _BodySizeLimitMiddleware
from app.registry import SessionRegistry


def _app(limit: int) -> FastAPI:
    app = FastAPI()
    app.add_middleware(_BodySizeLimitMiddleware, max_bytes=limit)

    @app.post("/echo")
    async def echo(request: Request):
        body = await request.body()  # forca a leitura do corpo (onde o middleware conta/aborta)
        return {"n": len(body)}

    return app


def test_body_limit_content_length():
    # Corpo com Content-Length: abaixo passa, acima e 413 (rejeitado antes do handler).
    c = TestClient(_app(10))
    assert c.post("/echo", content=b"x" * 5).status_code == 200
    assert c.post("/echo", content=b"x" * 50).status_code == 413


def test_body_limit_chunked_sem_content_length():
    # Corpo chunked (generator -> Transfer-Encoding: chunked, sem Content-Length): o check de
    # Content-Length nao pega; o contador no receive precisa abortar mesmo assim.
    c = TestClient(_app(10))

    def gen():
        for _ in range(50):
            yield b"x"

    assert c.post("/echo", content=gen()).status_code == 413


def test_registry_rename_migra_cache():
    reg = SessionRegistry()
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._jsonl_cache["old"] = "/p/abc.jsonl"
    reg.rename("old", "new")
    assert "old" not in SessionRegistry._jsonl_cache       # nome velho esquecido
    assert SessionRegistry._jsonl_cache["new"] == "/p/abc.jsonl"  # jsonl migrado pro novo
    # rename de nome SEM cache nao deve criar entrada None
    reg.rename("ghost", "new2")
    assert "new2" not in SessionRegistry._jsonl_cache
    SessionRegistry._jsonl_cache.clear()


def test_open_jsonl_rejeita_dir_irmao_com_mesmo_prefixo(monkeypatch):
    # _open_jsonl nao pode casar um fd que aponta pra dir IRMAO de mesmo prefixo de string
    # (projects-evil/ vs projects/) -> senao serviria o transcript de outra sessao.
    import app.registry as reg
    base = Path("/home/u/.claude/projects")
    evil = "/home/u/.claude/projects-evil/x.jsonl"  # prefixo de string casa, mas NAO e filho
    good = "/home/u/.claude/projects/sess/y.jsonl"
    monkeypatch.setattr(reg.os, "listdir", lambda p: ["0", "1"])
    monkeypatch.setattr(reg.os, "readlink", lambda p: evil if p.endswith("/0") else good)
    assert reg._open_jsonl(123, base) == good  # pula o irmao, acha o filho real
