"""Checks dos fixes batch 1-3: middleware global de body-size e migracao do cache no rename."""
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
