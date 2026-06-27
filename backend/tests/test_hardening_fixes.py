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


def test_read_from_nao_consome_linha_parcial(tmp_path):
    # awatch pode disparar no meio de um append -> a ultima linha vem sem \n. _read_from nao pode
    # consumi-la (senao a versao completa nunca seria relida). Deve ler so as completas e rebobinar.
    from app.transcript import TranscriptTailer
    f = tmp_path / "t.jsonl"
    completa = '{"type":"user","uuid":"u1","message":{"role":"user","content":"oi"}}\n'
    parcial = '{"type":"user","uuid":"u2","message":{"role":"user","content":"incompl'
    f.write_text(completa + parcial, encoding="utf-8")
    t = TranscriptTailer(f)
    evs, pos = t._read_from(0)
    assert [e.id for e in evs] == ["u1"]  # so a completa; a parcial ficou de fora
    # quando a 2a linha completa, a releitura a partir de `pos` a pega (nao foi consumida pela metade)
    f.write_text(completa + parcial + 'eto"}}\n', encoding="utf-8")
    evs2, _ = t._read_from(pos)
    assert [e.id for e in evs2] == ["u2"]


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
