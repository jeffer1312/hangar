"""Lista única de credenciais (app/credenciais.py) + apelido (app/apelidos.py).

Nada de rede nem de disco real: `list_config_dirs`, `engines.listar`, `_login_de` e
`cotas.listar_cotas` são trocados; o apelido escreve num `tmp_path` pela pasta compartilhada.
"""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from app import apelidos, contas, cotas, credenciais, engines, codex_contas


@pytest.fixture
def casa(tmp_path, monkeypatch):
    """Pasta compartilhada de mentira — é onde o mapa de apelidos é gravado."""
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    monkeypatch.setattr(codex_contas, "list_accounts", lambda: [])
    monkeypatch.setattr(contas, "compartilhado", lambda: tmp_path)
    return tmp_path


# ----------------------------------------------------------------------------------- apelidos


def test_apelido_grava_e_le(casa):
    assert apelidos.ler() == {}
    apelidos.definir("kimi:apikey", "Kimi")
    assert apelidos.ler() == {"kimi:apikey": "Kimi"}
    assert apelidos.de("kimi:apikey", "apikey") == "Kimi"
    assert apelidos.de("outro:x", "natural") == "natural"


def test_apelido_vazio_apaga(casa):
    apelidos.definir("a", "Nome")
    apelidos.definir("a", "   ")
    assert apelidos.ler() == {}


def test_arquivo_ilegivel_nao_derruba(casa):
    (casa / ".hangar-apelidos.json").write_text("{isto nao e json", encoding="utf-8")
    assert apelidos.ler() == {}
    # JSON válido do TIPO errado cai no mesmo lugar (precedente do statusline.read()).
    (casa / ".hangar-apelidos.json").write_text("[1,2]", encoding="utf-8")
    assert apelidos.ler() == {}


def test_apelido_tem_teto_de_tamanho(casa):
    apelidos.definir("a", "x" * 200)
    assert len(apelidos.ler()["a"]) == apelidos._MAX


# -------------------------------------------------------------------------------- lista única


def _monta(monkeypatch, *, dirs=(), motores=None, cotas_lista=()):
    monkeypatch.setattr(credenciais, "list_config_dirs", lambda: list(dirs))
    monkeypatch.setattr(engines, "listar", lambda: dict(motores or {}))
    monkeypatch.setattr(credenciais, "_login_de", lambda c: credenciais.EstadoLogin(estado="ok", loggedIn=True))
    monkeypatch.setattr(contas, "e_conta", lambda p: True)
    # `forcar=False` na assinatura: a lista aceita ?forcar=true (botão "atualizar" da tela),
    # que só repassa o flag pra cá — o mock recebe e ignora.
    monkeypatch.setattr(cotas, "listar_cotas", lambda forcar=False: list(cotas_lista))


def _dir(path, label, active=False):
    return SimpleNamespace(path=path, label=label, active=active)


def _cota(cid, pct=13.0, estado="lida", label="x"):
    return cotas.CotaConta(id=cid, label=label, provedor="claude", estado=estado,
                           janelas=[cotas.JanelaCota(rotulo="5h", pct=pct)] if estado == "lida" else [],
                           ts=1000.0)


def test_conta_do_claude_e_chave_saem_na_MESMA_lista(casa, monkeypatch):
    _monta(monkeypatch,
           dirs=[_dir("/home/u/.claude", "default", True)],
           motores={"kimi": {"label": "Kimi", "base_url": "https://api.kimi.com/coding/v1",
                             "api_key": "chave-de-exemplo-1234"}},
           cotas_lista=[_cota("claude:/home/u/.claude", 13.0), _cota("chave:kimi", 5.0)])
    linhas = credenciais.listar()
    assert [(c.tipo, c.nome) for c in linhas] == [("claude", "default"), ("chave", "Kimi")]
    assert linhas[0].ativa is True and linhas[0].path == "/home/u/.claude"
    assert linhas[1].usos == ["claude_code"]
    # Cota casada por id nos dois tipos — é o mesmo id da faixa do rodapé.
    assert [c.cota.janelas[0].pct for c in linhas] == [13.0, 5.0]


def test_a_chave_NUNCA_volta_inteira(casa, monkeypatch):
    _monta(monkeypatch, motores={"kimi": {"label": "Kimi", "base_url": "https://x/y",
                                          "api_key": "chave-de-exemplo-longa-4f2a"}})
    linha = credenciais.listar()[0]
    assert linha.chave_mascarada == "chave-d••••4f2a"
    assert "valor-longo" not in json.dumps(linha.model_dump())


def test_apelido_troca_o_nome_mas_guarda_o_original(casa, monkeypatch):
    _monta(monkeypatch, dirs=[_dir("/home/u/.claude-200-01", "200-01")])
    apelidos.definir("claude:/home/u/.claude-200-01", "PMédico 01")
    linha = credenciais.listar()[0]
    assert (linha.nome, linha.nome_natural, linha.apelido) == ("PMédico 01", "200-01", "PMédico 01")


def test_credencial_que_so_a_cota_conhece_aparece_na_lista(casa, monkeypatch):
    """O provider do Kimi vem do config.toml dele, não do cadastro do app. Se ele aparece na faixa
    do rodapé e não na tela, a tela mente sobre ser "todas as credenciais desta máquina"."""
    _monta(monkeypatch, cotas_lista=[_cota("kimi:apikey", 5.0, label="apikey")])
    linhas = credenciais.listar()
    assert [(c.id, c.tipo, c.usos) for c in linhas] == [("kimi:apikey", "chave", ["kimi_cli"])]
    assert linhas[0].nome_natural == "apikey"
    # provedor nativo do Kimi: o app lista, mas não é dele pra apagar
    assert linhas[0].gerenciada is False


def test_bloco_orfao_do_app_no_kimi_e_gerenciado(casa, monkeypatch):
    from app import agentes_sync
    monkeypatch.setattr(agentes_sync, "bloco_gerenciado", lambda cfg, nome: nome == "orfao")
    _monta(monkeypatch, cotas_lista=[_cota("kimi:orfao", 5.0, label="orfao"),
                                     _cota("kimi:apikey", 5.0, label="apikey")])
    assert {c.id: c.gerenciada for c in credenciais.listar()} == {"kimi:orfao": True, "kimi:apikey": False}


@pytest.mark.parametrize("method,status", [("oauth", "connected"), ("api_key", "connected"),
                                             ("none", "disconnected"), ("unknown", "unavailable")])
@pytest.mark.parametrize("with_quota", [True, False])
def test_credencial_do_codex_tambem_aparece(casa, monkeypatch, method, status, with_quota):
    cid = f"codex:{casa / '.codex'}"
    _monta(monkeypatch, cotas_lista=[_cota(cid)] if with_quota else [])
    account = codex_contas.Account("default", casa / ".codex", True)
    monkeypatch.setattr(codex_contas, "list_accounts", lambda: [account])
    apelidos.definir(cid, "Trabalho")
    snapshots = [{"id": "default", "auth": {"method": method, "status": status,
                  "email": "user@example.test" if method == "oauth" else None,
                  "plan": "pro" if method == "oauth" else None}}]
    rows = credenciais.listar(codex_snapshots=snapshots)
    assert len(rows) == 1
    row = rows[0]
    assert (row.id, row.tipo, row.codex_account, row.auth_method) == (cid, "codex", "default", method)
    assert row.nome == "Trabalho"
    assert (row.cota is not None) == with_quota
    assert row.login.loggedIn == (None if status == "unavailable" else status == "connected")
    if method == "oauth":
        assert (row.login.email, row.login.plano) == ("user@example.test", "pro")


def test_codex_sem_snapshot_nao_inventa_autenticacao(casa, monkeypatch):
    _monta(monkeypatch)
    monkeypatch.setattr(codex_contas, "list_accounts", lambda: [codex_contas.Account("default", casa / ".codex", True)])
    row = credenciais.listar()[0]
    assert row.auth_method == "unknown"
    assert row.login.estado == "indisponivel"


@pytest.mark.asyncio
async def test_endpoint_reusa_auth_publica_sem_esperar_preparo(casa, monkeypatch):
    _monta(monkeypatch)
    account = codex_contas.Account("default", casa / ".codex", True)
    monkeypatch.setattr(codex_contas, "list_accounts", lambda: [account])
    service = SimpleNamespace(
        preparation_status=lambda a: {"status": "running"},
        cached_auth=lambda a: {"method": "oauth", "status": "connected", "email": "x@example.test", "plan": "pro"},
    )
    request = SimpleNamespace(app=SimpleNamespace(state=SimpleNamespace(codex_contas_login=service)))
    rows = await credenciais.listar_endpoint(request)
    assert rows[0].auth_method == "oauth"


@pytest.mark.asyncio
@pytest.mark.parametrize("method,status", [("oauth", "connected"), ("none", "disconnected"),
                                           ("unknown", "unavailable")])
async def test_endpoint_le_identidade_nativa_sem_cota(casa, monkeypatch, method, status):
    from unittest.mock import AsyncMock

    _monta(monkeypatch)
    account = codex_contas.Account("default", casa / ".codex", True)
    monkeypatch.setattr(codex_contas, "list_accounts", lambda: [account])
    read = AsyncMock(return_value={"method": method, "status": status, "email": None, "plan": None})
    service = SimpleNamespace(preparation_status=lambda a: {"status": "ready"},
                              cached_auth=lambda a: None, read_auth=read)
    request = SimpleNamespace(app=SimpleNamespace(state=SimpleNamespace(codex_contas_login=service)))
    rows = await credenciais.listar_endpoint(request, forcar=True)
    read.assert_awaited_once_with(account, refresh=True)
    assert len(rows) == 1 and rows[0].auth_method == method and rows[0].cota is None


def test_conta_sem_cota_lida_continua_na_lista(casa, monkeypatch):
    """Sem leitura de limite a linha existe do mesmo jeito — some-la esconderia justo a conta que
    precisa de atenção."""
    _monta(monkeypatch, dirs=[_dir("/home/u/.claude", "default", True)],
           cotas_lista=[_cota("claude:/home/u/.claude", estado="indisponivel")])
    linha = credenciais.listar()[0]
    assert linha.cota.estado == "indisponivel" and linha.cota.janelas == []
