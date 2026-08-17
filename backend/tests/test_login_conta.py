"""Máquina de estados do login numa conta pelo app (Task 7) — por trás da janela escondida.

O contrato de I/O: TODA fala com o tmux é feita pelas funções privadas `_shell_criar` /
`_shell_digitar` / `_shell_ler` / `_shell_matar`, trocadas nos testes por texto de mentira.
O fluxo real (medido em 17/08): `claude auth login --claudeai` imprime a URL OAuth no pane e
espera "Paste code here if prompted >"; o código colado completa a autorização. A confirmação
NUNCA vem da aparência da tela — o login só é confirmado relendo o estado da conta via
`_estado_login` (a mesma fonte da Task 4). Em NENHUM caminho a janela escondida sobrevive.
"""
import threading
import time

import pytest

from app import conta_estado, login_conta, tmux


def _pane_mentira():
    return """Welcome to Claude Code!
? Choose the text style ...
> https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code
Paste code here if prompted >"""


class _Bateia:
    """Trocáveis do módulo, gravando o que foi chamado (sem rede, sem processo)."""

    def __init__(self, ler=None):
        self.criadas = []
        self.digitadas = []
        self.matadas = []
        self.ler_ = ler or (lambda: "Welcome to Claude Code!")
        self.falhar_criacao = False

    def criar(self, nome, cwd):
        if self.falhar_criacao:
            return None
        self.criadas.append(nome)
        return f"term-{nome}"

    def digitar(self, nome, texto):
        self.digitadas.append((nome, texto))

    def ler(self, nome):
        return self.ler_()

    def matar(self, nome):
        self.matadas.append(nome)
        # Depois de morta, a tentativa some: um cancelar concorrente não pode matar de novo.
        _tentativas.pop("conta-a", None)


@pytest.fixture
def bateia(monkeypatch):
    b = _Bateia()
    monkeypatch.setattr(login_conta, "_shell_criar", b.criar)
    monkeypatch.setattr(login_conta, "_shell_digitar", b.digitar)
    monkeypatch.setattr(login_conta, "_shell_ler", b.ler)
    monkeypatch.setattr(login_conta, "_shell_matar", b.matar)
    return b


@pytest.fixture(autouse=True)
def _limpa_estado(monkeypatch):
    # Estado de módulo (a janela da tentativa em voo) não pode vazar entre testes.
    monkeypatch.setattr(login_conta, "_tentativas", {}, raising=False)


# --------------------------------------------------------------------------- chamadas


def test_iniciar_cria_janela_e_digita_login(bateia):
    login_conta.iniciar("conta-a", "/home/u")
    assert bateia.criadas == ["login-conta-a"]
    assert bateia.digitadas == [("login-conta-a", "claude auth login --claudeai")]


def test_iniciar_falhou_na_criacao_nao_digita_e_devolve_erro(bateia):
    bateia.falhar_criacao = True
    with pytest.raises(RuntimeError):
        login_conta.iniciar("conta-a", "/home/u")
    assert bateia.digitadas == []
    assert bateia.matadas == []


def test_iniciar_ja_em_andamento_nao_duplica_janela(bateia):
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(RuntimeError):
        login_conta.iniciar("conta-a", "/home/u")
    assert bateia.criadas == ["login-conta-a"]


def test_cancelar_mata_a_janela(bateia):
    login_conta.iniciar("conta-a", "/home/u")
    login_conta.cancelar("conta-a")
    assert bateia.matadas == ["login-conta-a"]
    # Depois de cancelar, uma tentativa nova pode começar.
    login_conta.iniciar("conta-a", "/home/u")
    assert bateia.criadas == ["login-conta-a", "login-conta-a"]


def test_cancelar_sem_tentativa_e_noop(bateia):
    login_conta.cancelar("conta-a")
    assert bateia.matadas == []


# ------------------------------------------------------------------ leitura do passo


def test_le_endereco_de_autorizacao(bateia):
    bateia.ler_ = lambda: _pane_mentira()
    login_conta.iniciar("conta-a", "/home/u")
    passo = login_conta.passo("conta-a")
    assert passo["etapa"] == "aguardando"
    assert "https://claude.com/cai/oauth/authorize" in passo["url"]
    assert "client_id=9d1c250a" in passo["url"]


def test_sem_url_ainda_na_tela_passo_aguardando_sem_url(bateia):
    bateia.ler_ = lambda: "Welcome to Claude Code!"
    login_conta.iniciar("conta-a", "/home/u")
    passo = login_conta.passo("conta-a")
    assert passo["etapa"] == "aguardando"
    assert passo["url"] is None


def test_sem_tentativa_passo_devolve_idle(bateia):
    passo = login_conta.passo("conta-a")
    assert passo["etapa"] == "idle"


# --------------------------------------------------------------------- confirmação


def test_confirmar_digita_o_codigo_e_confirma_pela_releitura(bateia):
    # A confirmação é RELER o estado da conta — nunca a aparência da tela (requisito do
    # Step 1). Aqui a tela de mentira NUNCA mostra login; quem decide é o `_estado_login`.
    bateia.ler_ = lambda: "Paste code here if prompted >"
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123")  # sem estado fake: CLI real ausente no teste

    assert bateia.digitadas[-1] == ("login-conta-a", "CODE-123")
    # A janela só sai no fim (limpeza garantida em qualquer caminho).
    assert bateia.matadas == ["login-conta-a"]


def test_confirmar_sem_tentativa_devolve_erro(bateia):
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123")


def test_confirmar_espera_ate_a_conta_mostrar_logada(bateia):
    # Depois de digitar o código, o OAuth ainda está processando: a releitura devolve
    # deslogada (com estado ok) algumas vezes e a confirmação espera e relê de novo —
    # até a conta aparecer logada.
    respostas = iter([{"loggedIn": False}, {"loggedIn": False}, {"loggedIn": True,
                      "email": "u@exemplo.com", "subscriptionType": "max"}])

    def estado_fake(dir_conta):
        return conta_estado._estado_login(next(respostas))

    # Só para o laço: o poll real dorme; aqui o tick é curto.
    monkeypatch_poll(0.001)

    login_conta.iniciar("conta-a", "/home/u")
    resultado = login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake)
    assert resultado["ok"] is True
    assert resultado["email"] == "u@exemplo.com"
    assert resultado["plano"] == "max"
    # A janela morreu no fim do fluxo (caminho de sucesso).
    assert bateia.matadas == ["login-conta-a"]


def test_confirmar_estado_indisponivel_nao_vira_logado(bateia):
    # `loggedIn: null` NÃO é deslogado: é formato que não dá pra confiar. A confirmação
    # recusa e a janela NÃO sobrevive.
    def estado_fake(dir_conta):
        return conta_estado._estado_login(None)   # cli-indisponivel

    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake)
    assert bateia.matadas == ["login-conta-a"]


def test_confirmar_timeout_deixa_janela_morta(bateia):
    def estado_fake(dir_conta):
        return conta_estado._estado_login({"loggedIn": False})

    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(TimeoutError):
        login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake, timeout_s=0.05)
    assert bateia.matadas == ["login-conta-a"]


def test_cancelar_depois_do_confirmar_mata_janela(bateia):
    # O usuário pode cancelar DEPOIS de ter confirmado o código, enquanto a conta ainda
    # não relê logada — a janela não pode sobreviver.
    def estado_fake(dir_conta):
        return conta_estado._estado_login({"loggedIn": False})

    def cancela_em_thread():
        time.sleep(0.01)
        login_conta.cancelar("conta-a")

    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    t = threading.Thread(target=cancela_em_thread)
    t.start()
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake, timeout_s=5)
    t.join()
    assert bateia.matadas == ["login-conta-a"]


# -------------------------------------------------------------------------- helpers


def monkeypatch_poll(dt):
    # Reduz o sleep do poll de confirmação só nos testes que confirmam — a suíte não
    # espera o poll real de segundos.
    import app.login_conta as lc
    lc._POLL_S = dt
