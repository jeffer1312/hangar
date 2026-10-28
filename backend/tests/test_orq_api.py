"""Rotas de orquestração: política (GET/PUT) e papéis do grupo (GET/POST) e o kick-off do árbitro."""
import os
import pytest
from orq_contract_client import roles as orq_papeis
from orq_http_native import NativeClient, NativePlan as orq_start


@pytest.fixture
def cli(monkeypatch, tmp_path):
    return NativeClient(tmp_path, monkeypatch)


H = {"Authorization": "Bearer secret"}


def test_politica_put_e_get(cli, tmp_path):
    r = cli.get("/api/orquestracao/politica", headers=H)
    assert r.status_code == 200 and r.json()["politica"] == [] and len(r.json()["inventario"]) == 2
    r = cli.put("/api/orquestracao/politica/200-01", headers=H,
                json={"provider": "claude", "modelos": ["opus[1m]"], "trocar": False, "mtime": 0.0})
    assert r.status_code == 200, r.text
    pol = cli.get("/api/orquestracao/politica", headers=H).json()["politica"]
    assert pol[0]["conta"] == "200-01" and pol[0]["modelos"] == ["opus[1m]"] and pol[0]["trocar"] is False
    assert "`apikey` (kimi) — não liberada" in cli.policy.read_text(encoding="utf-8")
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


def test_papel_post_grava_sem_avisar_arbitro(cli, tmp_path):
    # Sem grupo: o registro pertence somente a esta sessão, sem time padrão.
    r = cli.get("/api/sessions/solta/orq", headers=H)
    assert r.status_code == 200 and r.json()["gid"].startswith("draft-") and r.json()["papeis"] == []
    assert r.json()["grouped"] is False and r.json()["session_prefix"] == "solta"
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
    assert r.status_code == 200 and r.json()["aviso"] == "proxima_sessao" and r.json()["arbitro"] is None
    mt = r.json()["mtime"]
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "executor", "sessao": "exe*", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "medium", "mtime": mt})
    assert r.status_code == 200, r.text
    assert r.json()["aviso"] == "proxima_sessao"
    assert not cli.enviados
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


def test_actual_group_remains_readable_without_identity(cli, monkeypatch):
    (cli.sidecars / "exec.json").unlink()
    r = cli.get("/api/sessions/exec/orq", headers=H)
    assert r.status_code == 200 and r.json()["grouped"] is True
    assert r.json()["gid"] == "g1" and r.json()["session_identity"] is None


def test_find_plan_ignora_exemplo_e_plano_terminado(tmp_path):
    """Exemplo do formato dentro de bloco de código não é plano, e plano com todo Step marcado —
    carimbado ou não — é trabalho feito: nenhum dos dois pode virar o plano do play."""
    import subprocess
    orq = orq_start._orq()
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    (tmp_path / "doc.md").write_text("````md\n```\n### Task 1: x\n```\n### Task 2: y\n````\n", encoding="utf-8")
    feito = "### Task 1: x\n- [x] **Step 1: y**\n"
    (tmp_path / "feito.orq.md").write_text(f"Preparado: ontem · sha {orq.plan_sha(feito)}\n{feito}", encoding="utf-8")
    assert orq.find_plan(tmp_path) == {"finished": str(tmp_path / "feito.orq.md")}
    (tmp_path / "aberto.md").write_text("### Task 1: x\n- [ ] **Step 1: y**\n", encoding="utf-8")
    assert orq.find_plan(tmp_path)["path"] == str(tmp_path / "aberto.md")


@pytest.mark.skipif(os.name == "nt", reason="a sonda de erro usa um executável shell POSIX")
def test_prontidao_nao_confunde_git_quebrado_com_sem_plano(tmp_path, monkeypatch):
    """Git que falha (timeout, dono duvidoso) não é "não há plano": o recado de planejadora avisa
    que não deu para ler, senão ela escreve um plano novo por cima de um que existe."""
    import subprocess
    orq = orq_start._orq()
    programs = tmp_path / "programs"
    programs.mkdir()
    git = programs / "git"
    git.write_text("#!/bin/sh\necho 'fatal: detected dubious ownership' >&2\nexit 128\n", encoding="utf-8")
    git.chmod(0o700)
    monkeypatch.setenv("PATH", str(programs))
    r = orq_start.readiness(str(tmp_path), False, True)
    assert r["phase"] == "planner" and any("dubious ownership" in p for p in r["problems"])
    assert "Não consegui ler tudo" in orq_start.kickoff(r, "/c/regras.md")
    sem = orq_start.readiness(None, False, True)
    assert sem["problems"] and "confira se já existe" in orq_start.kickoff(sem, "/c/regras.md")


def test_comecar_parte_do_passo_que_falta(cli, tmp_path):
    """O botão "Começar" nunca recusa por falta de plano: sem plano a PRÓPRIA sessão vira
    planejadora; com o plano carimbado e grupo, árbitra. A régua é o `find_plan` do orq.py."""
    import subprocess
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "árbitro", "sessao": "arb", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})

    r = cli.post("/api/sessions/exec/orq/comecar", headers=H, json={})
    assert r.status_code == 200 and r.json()["fase"] == "planner", r.text
    alvo, texto = cli.enviados[-1]
    assert alvo == "exec", "o kick-off tem de ir pra PRÓPRIA sessão, não pro árbitro da tabela"
    assert "PLANEJADORA" in texto and "orquestrar" in texto and "regras-g1.md" in texto

    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    corpo = "# Plano\n\n### Task 1: x\n- [ ] **Step 1: y**\n"
    orq = orq_start._orq()
    (tmp_path / "p1.orq.md").write_text(f"Preparado: hoje · sha {orq.plan_sha(corpo)}\n{corpo}", encoding="utf-8")
    assert cli.get("/api/sessions/exec/orq", headers=H).json()["prontidao"]["phase"] == "arbiter"
    r = cli.post("/api/sessions/exec/orq/comecar", headers=H, json={})
    assert r.status_code == 200 and r.json()["fase"] == "arbiter", r.text
    texto = cli.enviados[-1][1]
    assert "ÁRBITRO" in texto and str(tmp_path / "p1.orq.md") in texto


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


def test_papel_guarda_abertura_e_recusa_o_que_a_criacao_recusaria(cli, tmp_path, monkeypatch):
    import json
    (cli.base / "engines.json").write_text(json.dumps({"m1": {"api_key": "synthetic", "base_url": "https://example.invalid"}}), encoding="utf-8")
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    mt1 = cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0}).json()["mtime"]
    cli.put("/api/orquestracao/politica/apikey", headers=H, json={"provider": "kimi", "mtime": mt1})
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "árbitro", "sessao": "arb", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})
    base = {"papel": "executor", "sessao": "exe*", "provider": "claude", "conta": "200-01",
            "modelo": "opus[1m]", "esforco": "medium", "mtime": r.json()["mtime"]}
    r = cli.post("/api/sessions/exec/orq/papel", headers=H,
                 json={**base, "headless": True, "permissao": "bypassPermissions", "jev": True})
    assert r.status_code == 200, r.text
    ex = next(p for p in cli.get("/api/sessions/exec/orq", headers=H).json()["papeis"] if p["papel"] == "executor")
    assert ex["headless"] is True and ex["permissao"] == "bypassPermissions" and ex["jev"] is True
    mt = r.json()["mtime"]
    recusas = [({"provider": "kimi", "conta": "apikey", "modelo": "apikey/k3", "headless": True}, "erro_orq_headless_provider"),
               ({"motor": "nao-existe"}, "erro_motor_invalido"),
               ({"motor": "m1", "subagente": "sonnet"}, "erro_subagente_so_claude"),
               ({"permissao": "tudo"}, "erro_permissao_invalida")]
    for extra, codigo in recusas:
        r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={**base, "mtime": mt, **extra})
        assert r.status_code == 400 and r.json()["detail"]["code"] == codigo, (extra, r.text)


def test_salvar_papeis_nunca_acorda_o_arbitro(cli, tmp_path):
    """A linha nova vale na próxima sessão de cada papel: o árbitro lê a tabela ao abrir. Recado
    na hora mandava fechar sessão parada e fazer sucessão no meio de uma Task."""
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    r = cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "árbitro", "sessao": "arb", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})
    for avisar in (True, False):
        mt = cli.get("/api/sessions/exec/orq", headers=H).json()["mtime"]
        r = cli.post("/api/sessions/exec/orq/papeis", headers=H, json={
            "avisar": avisar, "mtime": mt, "papeis": [
                {"papel": "revisor", "sessao": "rev*", "provider": "claude", "conta": "200-01",
                 "modelo": "opus[1m]", "esforco": "high"}]})
        assert r.status_code == 200 and r.json()["aviso"] == "proxima_sessao", r.text
    assert cli.enviados == []


def test_sem_terminal_com_read_only_na_abertura_e_recusado(cli, tmp_path):
    mt0 = cli.get("/api/orquestracao/politica", headers=H).json()["mtime"]
    cli.put("/api/orquestracao/politica/200-01", headers=H, json={"provider": "claude", "mtime": mt0})
    cli.post("/api/sessions/exec/orq/papel", headers=H, json={
        "papel": "revisor", "sessao": "rev", "provider": "claude", "conta": "200-01",
        "modelo": "opus[1m]", "esforco": "high", "mtime": 0.0})
    # O árbitro escreve `--read-only` na célula de abertura; o painel não edita esse trecho.
    from dataclasses import replace
    regras = cli.rules
    texto = regras.read_text(encoding="utf-8")
    rev = next(x for x in orq_papeis.ler(texto) if x.papel == "revisor")
    regras.write_text(orq_papeis.escrever_papel(texto, replace(rev, abertura_extra="--read-only")), encoding="utf-8")
    antes = regras.read_text(encoding="utf-8")
    got = cli.get("/api/sessions/exec/orq", headers=H).json()
    r = cli.post("/api/sessions/exec/orq/papeis", headers=H, json={
        "mtime": got["mtime"], "papeis": [
            {"papel": "revisor", "sessao": "rev", "provider": "claude", "conta": "200-01",
             "modelo": "opus[1m]", "esforco": "high", "headless": True}]})
    assert r.status_code == 400 and r.json()["detail"]["code"] == "erro_orq_headless_read_only"
    assert regras.read_text(encoding="utf-8") == antes
