"""Rotas de orquestração: política (GET/PUT) e papéis do grupo (GET/POST) com recado ao árbitro."""
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import orq_papeis, orq_politica
from app.config import settings


@pytest.fixture
def cli(monkeypatch, tmp_path):
    settings.auth_token = "secret"
    import app.api as api_mod
    from app.api import app
    monkeypatch.setattr(orq_politica, "caminho", lambda: tmp_path / "orquestracao-contas.md")
    monkeypatch.setattr(orq_papeis, "regras_path", lambda gid: tmp_path / f"regras-{gid}.md")
    inv = [
        orq_politica.ContaInventario("200-01", "claude", "Rafael", "claude:/x/200-01",
                                     ({"id": "opus[1m]"}, {"id": "sonnet"}), False),
        orq_politica.ContaInventario("apikey", "kimi", "Kimi", "kimi:apikey", ({"id": "apikey/k3"},)),
    ]
    monkeypatch.setattr(api_mod, "_inventario", lambda: inv)
    monkeypatch.setattr(api_mod, "PairLink", lambda name: SimpleNamespace(
        get=lambda: {"peers": ["arb"], "gid": "g1"} if name in ("exec", "arb") else None))
    sessoes = [SimpleNamespace(name="arb", last_activity=1.0), SimpleNamespace(name="exec", last_activity=2.0)]
    monkeypatch.setattr(api_mod.registry, "list", lambda: sessoes)
    enviados = []
    monkeypatch.setattr(api_mod, "_send_one", lambda n, t: (enviados.append((n, t)) or {"ok": True, "delivered": True}))
    monkeypatch.setattr(orq_politica, "_niveis", lambda p, m: None)
    c = TestClient(app)
    c.enviados = enviados
    return c


H = {"Authorization": "Bearer secret"}


def test_politica_put_e_get(cli, tmp_path):
    r = cli.get("/api/orquestracao/politica", headers=H)
    assert r.status_code == 200 and r.json()["politica"] == [] and len(r.json()["inventario"]) == 2
    r = cli.put("/api/orquestracao/politica/200-01", headers=H,
                json={"provider": "claude", "modelos": ["opus[1m]"], "trocar": False, "mtime": 0.0})
    assert r.status_code == 200, r.text
    pol = cli.get("/api/orquestracao/politica", headers=H).json()["politica"]
    assert pol[0]["conta"] == "200-01" and pol[0]["modelos"] == ["opus[1m]"] and pol[0]["trocar"] is False
    assert "`apikey` (kimi) — não liberada" in (tmp_path / "orquestracao-contas.md").read_text()
    # mtime velho -> 409; modelo fora do catálogo -> 400; conta desconhecida -> 400
    assert cli.put("/api/orquestracao/politica/200-01", headers=H,
                   json={"provider": "claude", "mtime": 0.0}).status_code == 409
    mt = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    r = cli.put("/api/orquestracao/politica/200-01", headers=H,
                json={"provider": "claude", "modelos": ["gpt"], "mtime": mt})
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_orq_modelo_desconhecido"
    assert cli.put("/api/orquestracao/politica/nada", headers=H,
                   json={"provider": "claude", "mtime": mt}).json()["detail"]["code"] == "erro_orq_conta_desconhecida"
    # desligar tira da tabela
    r = cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "ligada": False, "mtime": mt})
    assert r.status_code == 200
    assert cli.get("/api/orquestracao/politica", headers=H).json()["politica"] == []


def test_papel_post_grava_e_avisa_arbitro(cli, tmp_path):
    # Sem grupo: a tela edita o time padrão (regras-padrao.md), sem árbitro pra avisar.
    r = cli.get("/api/sessions/solta/orq", headers=H)
    assert r.status_code == 200 and r.json()["gid"] == "padrao" and r.json()["papeis"] == []
    r = cli.get("/api/sessions/exec/orq", headers=H)
    assert r.status_code == 200 and r.json()["papeis"] == [] and r.json()["arbitro"] is None
    # política com tabela mas sem esta conta -> 400 (política VAZIA não proíbe; ver test_orq_politica)
    assert cli.put("/api/orquestracao/politica/apikey", headers=H,
                   json={"provider": "kimi", "mtime": 0.0}).status_code == 200
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "executor", "sessao": "exe*", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "medium", "mtime": 0.0})
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_orq_conta_nao_liberada"
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    # árbitro primeiro (sem árbitro na tabela -> sem_arbitro)
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "árbitro", "sessao": "arb", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})
    assert r.status_code == 200 and r.json()["aviso"] == "enviado" and r.json()["arbitro"] == "arb"
    mt = r.json()["mtime"]
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "executor", "sessao": "exe*", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "medium", "mtime": mt})
    assert r.status_code == 200, r.text
    assert r.json()["aviso"] == "enviado"
    nome, texto = cli.enviados[-1]
    assert nome == "arb" and "`executor`" in texto and "regras-g1.md" in texto and "TRABALHANDO" in texto
    got = cli.get("/api/sessions/exec/orq", headers=H).json()
    ex = next(p for p in got["papeis"] if p["papel"] == "executor")
    assert ex["viva"] == "exec" and ex["id_cota"].startswith("claude:")
    assert got["arbitro"] == "arb"
    # mtime velho -> 409; célula com '|' -> 400
    assert cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "executor", "provider": "claude", "conta": "200-01", "mtime": 0.0}).status_code == 409
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "x|y", "provider": "claude", "conta": "200-01", "mtime": got["mtime"]})
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_orq_celula_invalida"
