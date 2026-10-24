"""Golden do pedido de navegador (`POST /api/sessions/{name}/nav`) pelo Python, modo `python`.

Cada caso roda a rota de verdade (`api.abrir_nav_sessao`) com a sessão existindo e o bind dado, e
grava o status e o detalhe da recusa ou a url que o marcador guardou. Os arquivos de pendentes
passam pelo leitor do Python (`sse._nav_carregar` + `nav_vivos`). O Rust repete os casos em
`tests/contract_nav.rs`.

Uso, de backend/: uv run python tests/fixtures/contract/gen_nav.py
"""
import asyncio
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from fastapi import HTTPException  # noqa: E402
from app import account_bridge, api, sse  # noqa: E402
from app.config import settings  # noqa: E402

TOKEN = "tok/en+ç~_.-"
URLS = [
    "http://x", "https://x/a?b=1", "HTTP://X", "HtTpS://x", "x.com", "localhost:5173/#/a",
    "  http://x  ", "\x1chttp://x\x1f", "ftp://x", "//x", "   ",
    "/api/sessions/s/pages/abc_DEF-1", "/api/sessions/s%20t/pages/a", "/api/sessions/s/pages/a",
    "/api/sessions/outra/pages/a", "/api/sessions/s/pages/" + "a" * 129, "/api/sessions/s/pages/a b",
    "/x", "/api/sessions/s/pages/a?x=1",
]
BINDS = ["0.0.0.0", "::", "192.168.1.5", "127.0.0.1", "fe80::1"]


def run_url(name: str, url: str, bind: str) -> dict:
    stored = {}
    async def go():
        with patch.object(account_bridge, "owner_mode", lambda: "python"), \
             patch.object(api, "_session_exists", lambda _n: True), \
             patch.object(api, "nav_pendente", lambda n, u: stored.update(name=n, url=u)), \
             patch.object(api, "resolve_bind_ip", lambda _s: bind), \
             patch.object(settings, "port", 8765), patch.object(settings, "auth_token", TOKEN):
            try:
                await api.abrir_nav_sessao(name, api.NavBody(url=url), None)
            except HTTPException as e:
                return {"status": e.status_code, "detail": e.detail}
        return {"status": 200, "out": stored["url"]}
    return asyncio.run(go())


def run_file(raw: str) -> list:
    with tempfile.TemporaryDirectory() as d:
        arq = Path(d) / "_pendentes.json"
        arq.write_text(raw, encoding="utf-8")
        with patch.object(sse, "_nav_arquivo", lambda: arq):
            sse._NAV_MARCADORES.clear()
            sse._nav_mtime = None
            return sorted(sse.nav_vivos())


def main() -> None:
    url_cases = []
    for bind in BINDS:
        for url in URLS:
            name = "s t" if "s%20t" in url else "s"
            url_cases.append({"name": name, "bind": bind, "input": url, "token": TOKEN, **run_url(name, url, bind)})
    # Fixo: o golden não pode vencer com o tempo.
    future = 4_000_000_000.0
    files = [
        json.dumps({"a": {"url": "u", "ts": future}, "b": {"url": 1, "ts": future}, "c": {"url": "u"},
                    "d": {"url": "u", "ts": "1"}, "e": "x", "f": {"url": "u", "ts": 1.0}}),
        "[1]", "nao json", "{}", json.dumps({"g": {"url": "u", "ts": future, "extra": True}}),
    ]
    file_cases = [{"raw": raw, "live": run_file(raw)} for raw in files]
    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    out = {"origin_commit": commit, "command": "uv run python tests/fixtures/contract/gen_nav.py",
           "port": 8765, "urls": url_cases, "files": file_cases}
    dest = HERE / "nav" / "nav.json"
    dest.parent.mkdir(exist_ok=True)
    dest.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{len(url_cases)} urls, {len(file_cases)} arquivos -> {dest}")


if __name__ == "__main__":
    main()
