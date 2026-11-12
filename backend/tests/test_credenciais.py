import pytest

from app import apelidos, contas, codex_contas


@pytest.fixture
def casa(tmp_path, monkeypatch):
    """Pasta compartilhada de mentira — é onde o mapa de apelidos é gravado."""
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    # A pasta existe: os testes daqui supõem conta Codex na máquina (sem ela e sem o CLI, a
    # padrão some da tela — ver codex_contas.list_visible_accounts).
    (tmp_path / ".codex").mkdir()
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
