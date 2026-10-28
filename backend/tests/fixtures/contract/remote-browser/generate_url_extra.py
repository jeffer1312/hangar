"""Caracteriza tipos de sidecar não cobertos pelo corpus original, somente no Python da base."""
import argparse
import ast
import hashlib
import io
import json
import logging
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

from generate_url_commands import SOURCE, REPO, clean_environment

HERE = Path(__file__).resolve().parent


def capture(root):
    sys.path.insert(0, str(root / "reference/backend"))
    from fastapi import Depends, FastAPI
    from fastapi.testclient import TestClient
    from app.auth import require_auth

    assert Path.home() == root / "home"
    app = FastAPI()
    source = root / "reference/backend/app/api.py"
    text = source.read_text(encoding="utf-8")
    handler = next(n for n in ast.parse(text).body
                   if isinstance(n, ast.FunctionDef) and n.name == "navegador_da_sessao")
    first = min(handler.lineno, *(decorator.lineno for decorator in handler.decorator_list))
    original_get = "".join(text.splitlines(keepends=True)[first - 1:handler.end_lineno])
    scope = {"app": app, "Depends": Depends, "require_auth": require_auth,
             "Path": Path, "json": json, "_log": logging.getLogger("app.api")}
    exec(compile(ast.fix_missing_locations(ast.Module(body=[handler], type_ignores=[])), str(source), "exec"), scope)
    nav = Path.home() / ".hangar/nav"
    nav.mkdir(parents=True)
    inputs = [
        ("url_bigint_float_order", r'{"chave":"session","url":{"big":123456789012345678901234567890,"float":1.0,"order":{"z":1,"a":2}}}'),
        ("url_lone_surrogate", r'{"chave":"session","url":"\ud800"}'),
        ("key_lone_surrogate_suffix", r'{"chave":"\ud800::session","url":"u"}'),
        ("url_nan", r'{"chave":"session","url":NaN}'),
        ("unused_nan", r'{"chave":"session","url":"u","unused":NaN}'),
        ("url_infinite_float", r'{"chave":"session","url":1e999}'),
        ("key_null", r'{"chave":null,"url":"u"}'),
        ("key_number", r'{"chave":12,"url":"u"}'),
        ("key_nan", r'{"chave":NaN,"url":"u"}'),
        ("json_bom", "\ufeff" + r'{"chave":"session","url":"u"}'),
        ("url_float_rounding", r'{"chave":"session","url":0.12345678901234567890123456789}'),
        ("url_float_exponent", r'{"chave":"session","url":1e-7}'),
        ("url_private_unicode", '{"chave":"session","url":"\U0010f800"}'),
    ]
    cases = []
    with TestClient(app, client=("127.0.0.1", 1234), raise_server_exceptions=False) as client:
        for name, raw in inputs:
            (nav / "case.json").write_text(raw, encoding="utf-8")
            response = client.get("/api/sessions/session/navegador", headers={"authorization": "Bearer contract-owner-token"})
            cases.append({"name": name, "raw": raw, "status": response.status_code, "body": response.text})
        response = client.head("/api/sessions/session/navegador", headers={"authorization": "Bearer contract-owner-token"})
        head = {"status": response.status_code, "body": response.text,
                "headers": {key: response.headers[key] for key in ("allow", "content-type", "content-length")}}
    return {"source_commit": SOURCE,
            "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_url_extra.py --output backend/tests/fixtures/contract/remote-browser/url-extra.json",
            "python_version": sys.version.split()[0], "platform": sys.platform,
            "original_get": {"source": original_get, "sha256": hashlib.sha256(original_get.encode("utf-8")).hexdigest()},
            "head": head,
            "seam": "GET original completo por AST e autenticação FastAPI real, exportados por git archive da base; corpo HTTP literal.",
            "limits": ["Sem lifespan, porteiros Rust ou navegador gráfico.", "Erros não tratados mantêm 500 Internal Server Error, sem correção do comportamento Python.", "HOME, contas, configuração e cwd temporários, antes dos imports; nenhum servidor externo."],
            "cases": cases}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=False)
    parser.add_argument("--capture", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.capture:
        print(json.dumps(capture(args.capture), ensure_ascii=False, indent=2))
        return
    if args.output is None:
        parser.error("--output é obrigatório; nunca substitui referência implicitamente")
    with tempfile.TemporaryDirectory(prefix="hangar-url-extra-") as temporary:
        root = Path(temporary)
        (root / "home").mkdir()
        (root / "workspace").mkdir()
        reference = root / "reference"
        reference.mkdir()
        archive = subprocess.run(["git", "archive", "--format=tar", SOURCE, "backend/app"], cwd=REPO,
                                 check=True, capture_output=True, timeout=30)
        with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as bundle:
            bundle.extractall(reference, filter="data")
        child = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--capture", str(root)],
                               env=clean_environment(root), cwd=root / "workspace", capture_output=True,
                               timeout=30)
        if child.returncode:
            sys.stderr.write(child.stderr.decode("utf-8", errors="replace"))
            child.check_returncode()
        args.output.write_text(json.dumps(json.loads(child.stdout), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
