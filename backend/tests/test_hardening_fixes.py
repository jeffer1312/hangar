"""Checks do middleware de body-size."""
from fastapi import FastAPI, Request
from fastapi.testclient import TestClient
from app.api import _BodySizeLimitMiddleware


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
