"""Golden da faixa e dos avisos de mod de uma sessão Claude sem entrada no Rust (aberta à mão).

Cada passo manda um `/api/plugin/ui` ou `/api/plugin/toast` às rotas do Python e grava o status e o
evento que o app receberia (`plugin_ui` pelo `band_json` quando a versão muda; `plugin_toast` pelo
`toasts_after`, com o relógio parado). O Rust repete os passos na sessão sem entrada no `Mods` e compara
o JSON (não os bytes: o `json.dumps` do Python põe espaço depois de `:` e `,`).

Uso, de backend/: uv run python tests/fixtures/contract/gen_plugin_band.py
"""
import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

from app import plugin_bridge as pb  # noqa: E402

NAME = "solta"
PANE = {"id": "a", "title": "Título ✓", "placement": "dock", "columns": 58,
        "tree": {"type": "Box", "w": 1.0, "n": 12345678901234}, "data": {"x": 1}}
STEPS = [
    ("ui", "primeira faixa, painel com data e tree", {"above": {"type": "Text", "t": "olá"}, "columns": 87, "panes": [PANE]}),
    ("ui", "reenvio igual", {"above": {"type": "Text", "t": "olá"}, "columns": 87, "panes": [PANE]}),
    ("ui", "só a largura muda", {"above": {"type": "Text", "t": "olá"}, "columns": 90, "panes": [PANE]}),
    ("ui", "painel com padrões", {"above": None, "panes": [{"id": "b"}]}),
    ("ui", "faixa vazia", {"above": None, "panes": []}),
    ("ui", "sem campos", {}),
    ("ui", "above lista", {"above": [1], "panes": []}),
    ("ui", "id vazio", {"panes": [{"id": ""}]}),
    ("ui", "id longo", {"panes": [{"id": "p" * 65}]}),
    ("ui", "token errado", {"above": None, "panes": [], "_token": "x"}),
    ("toast", "aviso com prazo", {"text": "aviso", "timeoutMs": 4000, "plugin": "m"}),
    ("toast", "sem prazo", {"text": "sem prazo"}),
    ("toast", "prazo zero", {"text": "zero", "timeoutMs": 0}),
    ("toast", "prazo negativo", {"text": "negativo", "timeoutMs": -5}),
    ("toast", "prazo curto", {"text": "curto", "timeoutMs": 500}),
    ("toast", "prazo enorme", {"text": "enorme", "timeoutMs": 10**9}),
    ("toast", "prazo fracionado", {"text": "fração", "timeoutMs": 2500.7}),
    ("toast", "só espaços", {"text": "   "}),
    ("toast", "texto e mod longos", {"text": "é" * 2100, "plugin": "m" * 70}),
    ("toast", "mod nulo", {"text": "nulo", "plugin": None}),
    ("toast", "sem texto", {"plugin": "m"}),
    ("toast", "token errado", {"text": "x", "_token": "x"}),
]


def main() -> None:
    app = FastAPI()
    app.include_router(pb.plugin_router)
    client = TestClient(app)
    out = []
    with patch.object(pb.time, "monotonic", return_value=1000.0):
        version, seen = 0, 0
        for route, title, body in STEPS:
            body = dict(body)
            token = body.pop("_token", None) or pb.mint(NAME)
            r = client.post(f"/api/plugin/{route}", json={"sessao": NAME, "token": token, **body})
            emits = None
            if route == "ui":
                now = pb.band(NAME)[0]
                if now != version:
                    version, emits = now, json.loads(pb.band_json(NAME))
            else:
                seen, fresh = pb.toasts_after(NAME, seen)
                if fresh:
                    emits = [{k: v for k, v in t.items() if k != "id"} for t in fresh]
            out.append({"route": route, "title": title, "body": body, "token_ok": token == pb.mint(NAME),
                        "status": r.status_code, "emits": emits})
    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    dest = HERE / "plugin_band" / "cases.json"
    dest.parent.mkdir(exist_ok=True)
    dest.write_text(json.dumps({"commit": commit, "command": "uv run python tests/fixtures/contract/gen_plugin_band.py",
                                "session": NAME, "steps": out}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
