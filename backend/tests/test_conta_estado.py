"""Estado de login e do último limite de uma conta — por trás da CLI `claude auth status`.

A I/O (subprocess) mora em `_auth_status` e é trocada nos testes; o parsing
(`_parse_auth_status`) e a decisão de estado (`_estado_login`) são lógica pura em volta
dela (precedente: `engine_probe._buscar`). A leitura do sidecar de limite é disco local
(`_limite`), exercitada com `tmp_path` de verdade — sem rede, sem processo.
"""
import json
from pathlib import Path

from app import conta_estado


def test_conectada_devolve_email_e_plano():
    # Formato medido em 16/08: `claude auth status --json` respeita CLAUDE_CONFIG_DIR e
    # responde sem sessão viva.
    bruto = {"loggedIn": True, "authMethod": "claude.ai", "apiProvider": "firstParty",
             "email": "dev@example.com", "orgId": "x", "orgName": "y",
             "subscriptionType": "max"}
    login = conta_estado._estado_login(bruto)
    assert login.estado == "ok"
    assert login.loggedIn is True
    assert login.email == "dev@example.com"
    assert login.plano == "max"


def test_deslogada_e_estado_ok_sem_email():
    login = conta_estado._estado_login({"loggedIn": False})
    assert login.estado == "ok"
    assert login.loggedIn is False
    assert login.email is None
    assert login.plano is None


def test_formato_desconhecido_nao_vira_deslogada():
    # Sem o campo `loggedIn` não se afirma "deslogada": vira estado nomeado.
    login = conta_estado._estado_login({"authMethod": "api_key"})
    assert login.estado == "indisponivel"
    assert login.motivo == "formato-desconhecido"
    assert login.loggedIn is None


def test_cli_falhou_vira_estado_nomeado():
    login = conta_estado._estado_login(None)
    assert login.estado == "indisponivel"
    assert login.motivo == "cli-indisponivel"


def test_auth_status_sem_cli_nao_levanta(monkeypatch):
    # `claude` não está no PATH: a chamada externa falha sem exceção escapando pra rota.
    def _boom(*a, **k):
        raise FileNotFoundError("claude")

    monkeypatch.setattr(conta_estado.subprocess, "run", _boom)
    assert conta_estado._auth_status(Path("/tmp/x")) is None


def test_auth_status_rc_nao_zero_e_none(monkeypatch):
    class R:
        returncode = 1
        stdout = ""
        stderr = "erro qualquer"

    monkeypatch.setattr(conta_estado.subprocess, "run", lambda *a, **k: R())
    assert conta_estado._auth_status(Path("/tmp/x")) is None


def test_saida_que_nao_e_json_nao_quebra_listagem():
    # Precedente statusline.read(): saída não-JSON e JSON válido do tipo errado não podem
    # derrubar a resolução de estado — aqui viram "indisponivel", nunca exceção.
    assert conta_estado._parse_auth_status("isto não é json") is None
    assert conta_estado._parse_auth_status("[]") is None
    assert conta_estado._parse_auth_status("null") is None


def test_limite_lido_com_linha_e_idade(tmp_path):
    pasta = tmp_path / ".claude-pocket-status"
    pasta.mkdir()
    (pasta / "s.json").write_text(json.dumps({"line": "⚡5h:3% 📅7d:1%", "ts": 1000.0}))
    limite = conta_estado._limite(tmp_path)
    assert limite.estado == "lido"
    assert limite.linha == "⚡5h:3% 📅7d:1%"
    assert limite.ts == 1000.0
    assert limite.idade_s > 0


def test_limite_sem_leitura_e_explicito(tmp_path):
    # Pasta .claude-pocket-status nem existe: nada foi lido ainda. Não é zero nem ausente —
    # é um estado nomeado.
    limite = conta_estado._limite(tmp_path)
    assert limite.estado == "sem_leitura"
    assert limite.linha is None
    assert limite.idade_s is None


def test_limite_pega_o_mais_recente(tmp_path):
    pasta = tmp_path / ".claude-pocket-status"
    pasta.mkdir()
    (pasta / "a.json").write_text(json.dumps({"line": "velha", "ts": 1.0}))
    (pasta / "b.json").write_text(json.dumps({"line": "nova", "ts": 2.0}))
    limite = conta_estado._limite(tmp_path)
    assert limite.estado == "lido"
    assert limite.linha == "nova"
    assert limite.ts == 2.0


def test_limite_nao_quebra_com_sidecar_lixo(tmp_path):
    # Sidecar ilegível ou do tipo errado não derruba a listagem da conta (mesma regra do
    # statusline.read()): o válido continua sendo lido.
    pasta = tmp_path / ".claude-pocket-status"
    pasta.mkdir()
    (pasta / "lixo.json").write_text("não é json")
    (pasta / "errado.json").write_text("[1, 2]")
    (pasta / "bom.json").write_text(json.dumps({"line": "⚡5h:3%", "ts": 5.0}))
    limite = conta_estado._limite(tmp_path)
    assert limite.estado == "lido"
    assert limite.linha == "⚡5h:3%"


def test_email_ilegivel_nao_e_carimbado_como_bom():
    """`errors="replace"` na leitura da CLI transforma byte ruim em `�` — e o campo seguia como se
    fosse o endereço da pessoa, com a conta marcada `ok`.

    Some o campo, não a conta: `loggedIn` é bool e não sofre do problema (byte ruim DENTRO da
    estrutura quebraria o `json.loads`, que já vira `indisponivel`), e derrubar tudo pra
    `indisponivel` custaria o botão Entrar por causa de um campo de texto.
    """
    login = conta_estado._estado_login(
        {"loggedIn": True, "email": "jo�o@exemplo.com", "subscriptionType": "max"})
    assert login.estado == "ok"
    assert login.loggedIn is True
    assert login.email is None
    assert login.plano == "max"          # o campo bom ao lado do ruim continua valendo


def test_plano_ilegivel_some_sozinho():
    login = conta_estado._estado_login(
        {"loggedIn": True, "email": "dev@example.com", "subscriptionType": "m�x"})
    assert (login.email, login.plano) == ("dev@example.com", None)