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
    # `cwd` no stub porque o /orq/comecar procura o plano da PASTA da sessão.
    sessoes = [SimpleNamespace(name="arb", last_activity=1.0, cwd=str(tmp_path)),
               SimpleNamespace(name="exec", last_activity=2.0, cwd=str(tmp_path))]
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


def test_comecar_recusa_sem_plano_e_manda_kickoff_com_ele(cli, tmp_path, monkeypatch):
    """O botão "Começar" acorda a PRÓPRIA sessão como árbitra. Sem plano ele recusa com motivo: o
    árbitro despacha Tasks do plano, e acordar alguém sem ter o que despachar é pior que nada."""
    import app.api as api_mod
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "árbitro", "sessao": "arb", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})

    monkeypatch.setattr(api_mod, "plan_progress", lambda cwd: None)
    r = cli.post("/api/sessions/exec/orq/comecar", headers=H, json={})
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_orq_sem_plano"

    plano = SimpleNamespace(name="p1", path="/tmp/p1.md", done=2, total=9, task_idx=1, task_total=3)
    monkeypatch.setattr(api_mod, "plan_progress", lambda cwd: plano)
    antes = len(cli.enviados)
    r = cli.post("/api/sessions/exec/orq/comecar", headers=H, json={})
    assert r.status_code == 200, r.text
    assert len(cli.enviados) == antes + 1
    alvo, texto = cli.enviados[-1]
    assert alvo == "exec", "o kick-off tem de ir pra PRÓPRIA sessão, não pro árbitro da tabela"
    assert "ÁRBITRO" in texto and "orquestrar" in texto
    assert "/tmp/p1.md" in texto and "regras-g1.md" in texto


def test_remover_uma_conta_do_rodizio_nao_leva_as_outras(cli, tmp_path):
    """Remover pelo nome do papel sozinho apagaria a linha errada num papel que reveza: a chave é
    papel+vez. Remover não avisa o árbitro (quem mexe na fila mexe em várias linhas seguidas)."""
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    r = cli.post("/api/sessions/exec/orq/papeis", headers=H, json={
        "avisar": False, "mtime": 0.0, "papeis": [
            {"papel": "revisor", "sessao": "rev*", "provider": "claude", "conta": "200-01", "modelo": "opus[1m]", "esforco": "high", "vez": "1"},
            {"papel": "revisor", "sessao": "rev*", "provider": "claude", "conta": "200-01", "modelo": "sonnet", "esforco": "low", "vez": "2"},
            {"papel": "revisor", "sessao": "rev*", "provider": "claude", "conta": "200-01", "modelo": "opus[1m]", "esforco": "medium", "vez": "3"}]})
    assert r.status_code == 200, r.text
    mt = r.json()["mtime"]
    antes = len(cli.enviados)

    r = cli.request("DELETE", "/api/sessions/exec/orq/papel", headers=H,
                    json={"papel": "revisor", "vez": "2", "mtime": mt})
    assert r.status_code == 200, r.text
    revisores = [p for p in r.json()["papeis"] if p["papel"] == "revisor"]
    assert [p["vez"] for p in revisores] == ["1", "3"], "removeu a linha errada"
    assert len(cli.enviados) == antes, "remover mandou recado ao árbitro"

    # Linha que não existe é 404 legível, não um sucesso calado que não removeu nada.
    assert cli.request("DELETE", "/api/sessions/exec/orq/papel", headers=H,
                       json={"papel": "revisor", "vez": "9", "mtime": r.json()["mtime"]}).status_code == 404


def test_papeis_salvar_sem_avisar_grava_e_nao_acorda_o_arbitro(cli, tmp_path):
    """`avisar: false` é o "salvar e continuar montando o time": o contrato tem de ficar gravado, e
    o árbitro NÃO pode receber recado nenhum — antes, cada papel salvo o acordava com meia
    configuração. Sem o campo, o comportamento antigo (avisar) continua valendo."""
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "árbitro", "sessao": "arb", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})
    mt = r.json()["mtime"]
    antes = len(cli.enviados)

    r = cli.post("/api/sessions/exec/orq/papeis", headers=H, json={
        "avisar": False, "mtime": mt, "papeis": [
            {"papel": "revisor", "sessao": "rev*", "provider": "claude", "conta": "200-01",
             "modelo": "opus[1m]", "esforco": "high"}]})
    assert r.status_code == 200, r.text
    assert r.json()["aviso"] == "nao_avisado"
    assert len(cli.enviados) == antes, "salvar sem avisar mandou recado ao árbitro"
    # Gravado de verdade: a linha tem de aparecer na leitura seguinte.
    got = cli.get("/api/sessions/exec/orq", headers=H).json()
    assert any(p["papel"] == "revisor" for p in got["papeis"])

    r = cli.post("/api/sessions/exec/orq/papeis", headers=H, json={
        "mtime": got["mtime"], "papeis": [
            {"papel": "executor", "sessao": "exe*", "provider": "claude", "conta": "200-01",
             "modelo": "opus[1m]", "esforco": "medium"}]})
    assert r.status_code == 200 and r.json()["aviso"] == "enviado"
    assert len(cli.enviados) == antes + 1
