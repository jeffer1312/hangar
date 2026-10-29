"""Captura métodos, precedência e barras nas rotas originais, sem executar handlers."""
import ast
import json
from pathlib import Path
import subprocess
from fastapi import FastAPI
from fastapi.testclient import TestClient

ROOT = Path(__file__).resolve().parents[5]
app = FastAPI()
table = []
async def endpoint():
    return {"fixture": True}
for source, prefix in (("api.py", ""), ("credenciais.py", "/api/credenciais")):
    for node in ast.parse((ROOT / "backend/app" / source).read_text()).body:
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for decorator in node.decorator_list:
            if not isinstance(decorator, ast.Call) or not isinstance(decorator.func, ast.Attribute):
                continue
            if decorator.func.attr not in ("get", "post", "put", "delete") or not decorator.args:
                continue
            path = prefix + ast.literal_eval(decorator.args[0])
            if not (path.startswith("/api/engines") or path in ("/api/providers", "/api/credenciais/sincronizar", "/api/credenciais/apelido", "/api/credenciais/kimi/{nome}")):
                continue
            method = decorator.func.attr.upper()
            table.append([path, method])
            app.add_api_route(path, endpoint, methods=[method])
client = TestClient(app, follow_redirects=False)
cases = []
paths = ["/api/providers", "/api/providers/", "/api/engines", "/api/engines/", "/api/engines/motor", "/api/engines/motor/",
         "/api/engines/modelos", "/api/engines/cliproxy", "/api/engines/cliproxy/management-key",
         "/api/engines/cliproxy/management-key/", "/api/engines/ausente/extra", "/api/engines//",
         "/api/credenciais/sincronizar", "/api/credenciais/apelido", "/api/credenciais/kimi/motor",
         "/api/credenciais/kimi/motor/", "/api/engines/motor%2Foutro"]
for path in paths:
    for method in ("GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "TRACE"):
        response = client.request(method, path)
        cases.append({"path": path, "method": method, "status": response.status_code,
                      "headers": {key: value for key, value in response.headers.items() if key in ("allow", "location")},
                      "body": response.text})
Path(__file__).with_name("methods.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    "command": "uv run --with httpx python tests/fixtures/contract/engines/generate_methods.py",
    "table": table, "cases": cases,
}, ensure_ascii=True, indent=2) + "\n")
