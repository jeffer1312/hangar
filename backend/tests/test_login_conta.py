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
from pathlib import Path

import pytest

from app import conta_estado, login_conta, tmux


def _pane_mentira():
    return """Welcome to Claude Code!
? Choose the text style ...
> https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code
Paste code here if prompted >"""

# B2 — uma URL OAuth REAL passa de 80 colunas e o CLI a quebra na margem da janela
# escondida (medido: /login/passo devolvia "https://claude.com/cai/oauth/authorize?code=t").
# O capture-pane cru devolve a quebra como \n; com -J (juntar) vem inteira, com o state no fim.
_URL_QUEBRADA = ("Welcome to Claude Code!\n"
                 "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize"
                 "?code=true&client_id=9d1c250a-\n"
                 "e61b-44d9-88ed-5944d1962f5e&response_type=code&code_challenge=xyz&state=abc\n"
                 "Paste code here if prompted >")
_URL_JUNTA = ("Welcome to Claude Code!\n"
              "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize"
              "?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"
              "&response_type=code&code_challenge=xyz&state=abc\n"
              "Paste code here if prompted >")


class _Bateia:
    """Trocáveis do módulo, gravando o que foi chamado (sem rede, sem processo)."""

    def __init__(self, ler=None):
        self.criadas = []
        self.vivas = []          # janelas de mentira que EXISTEM (criadas e não mortas)
        self.eventos = []        # ordem real de matar:/criar: (prova de ORDEM do B3)
        self.config_dirs = []
        self.digitadas = []
        self.matadas = []
        self.enters = []
        self.ler_ = ler or (lambda: "Welcome to Claude Code!")
        self.falhar_criacao = False

    def criar(self, nome, cwd, config_dir=None):
        if self.falhar_criacao:
            return None
        alvo = f"term-{nome}"
        self.criadas.append(nome)
        self.vivas.append(alvo)      # a sessao que EXISTE e o retorno (term-<chave>)
        self.eventos.append(f"criar:{nome}")
        self.config_dirs.append(config_dir)
        return alvo

    def digitar(self, nome, texto):
        self.digitadas.append((nome, texto))

    def enviar_enter(self, nome):
        self.enters.append(nome)

    def submeter(self, nome, texto):
        self.digitar(nome, texto)
        self.enviar_enter(nome)

    def ler(self, nome):
        return self.ler_()

    def matar(self, nome):
        if nome not in self.vivas:
            # Nunca existiu: no-op, igual ao kill_session idempotente — sem isto o B3
            # (matar sobra ANTES de criar) apareceria como morte a cada iniciar.
            return
        self.vivas.remove(nome)
        self.eventos.append(f"matar:{nome}")
        self.matadas.append(nome)
        # Depois de morta, a tentativa some: um cancelar concorrente não pode matar de novo.
        # Referencia ao MODULO: o nome cru levantava NameError engolido pelo try/except do
        # _limpar (o pop nunca tinha rodado de verdade) — corrigido aqui.
        login_conta._tentativas.pop("conta-a", None)


@pytest.fixture
def bateia(monkeypatch):
    b = _Bateia()
    monkeypatch.setattr(login_conta, "_shell_criar", b.criar)
    monkeypatch.setattr(login_conta, "_shell_digitar", b.digitar)
    monkeypatch.setattr(login_conta, "_shell_submeter", b.submeter)
    monkeypatch.setattr(tmux, "send_keys", b.enviar_enter)
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
    # A chave pedida ao tmux é `login-conta-a`; o alvo REAL usado em digitar/Enter é o
    # retorno (`term-login-conta-a`) — o duplo devolve o que a primitiva devolve (B2).
    assert bateia.criadas == ["login-conta-a"]
    assert bateia.config_dirs == ["/home/u"]
    assert bateia.digitadas == [("term-login-conta-a", "claude auth login --claudeai")]
    assert bateia.enters == ["term-login-conta-a"]


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

def test_iniciar_mata_sobra_de_janela_antes_de_criar(bateia):
    # B3 — um backend que caiu no meio de uma tentativa deixa a janela VIVA no servidor
    # do tmux (a sessao sobrevive ao processo). Reatar traria um `claude auth login`
    # parado no prompt do codigo; o iniciar mata a sobra ANTES de criar a janela nova.
    bateia.vivas.append("term-login-conta-a")   # sobra de um backend morto
    login_conta.iniciar("conta-a", "/home/u")
    # ORDEM (não só ocorrência): matar a sobra vem ANTES de criar a janela nova.
    assert bateia.eventos == ["matar:term-login-conta-a", "criar:login-conta-a"]
    assert bateia.matadas == ["term-login-conta-a"]


def test_cancelar_mata_a_janela(bateia):
    login_conta.iniciar("conta-a", "/home/u")
    login_conta.cancelar("conta-a")
    assert bateia.matadas == ["term-login-conta-a"]
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

def test_passo_une_url_quebrada_na_margem(monkeypatch):
    # B2 — a URL OAuth passa de 80 colunas da janela escondida e o CLI a quebra na margem;
    # sem o -J o capture cru devolve a quebra como \n e o link da tela morre no meio
    # (medido: /login/passo devolvia "https://claude.com/cai/oauth/authorize?code=t").
    # O _shell_ler REAL pede o -J à primitiva e o passo devolve a URL INTEIRA, com o
    # state no fim.
    juntar_usado = {}

    def capture_fake(nome, lines=200, cores=False, juntar=False):
        juntar_usado["juntar"] = juntar
        return _URL_JUNTA if juntar else _URL_QUEBRADA

    monkeypatch.setattr(tmux, "capture_pane", capture_fake)
    monkeypatch.setattr(login_conta, "_shell_criar",
                        lambda nome, cwd, config_dir=None: f"term-{nome}")
    monkeypatch.setattr(login_conta, "_shell_submeter", lambda nome, texto: None)
    monkeypatch.setattr(login_conta, "_shell_matar", lambda nome: None)

    login_conta.iniciar("conta-a", "/home/u/.claude-conta-a")
    passo = login_conta.passo("conta-a")
    assert passo["etapa"] == "aguardando"
    assert passo["url"] == ("https://claude.com/cai/oauth/authorize?code=true&client_id="
                            "9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code"
                            "&code_challenge=xyz&state=abc")
    # O _shell_ler de verdade pediu o -J à primitiva — não é o duplo que decide.
    assert juntar_usado["juntar"] is True


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
    # Com `estado_fake` (a porta de teste do leitor real): a releitura devolve deslogada e
    # o teto curto estoura. O caminho sem estado_fake lê a CLI REAL e é da fumaça, não do
    # teste de unidade (B1: com o caminho de mentira a CLI criaria diretório fora da árvore
    # do repo).
    bateia.ler_ = lambda: "Paste code here if prompted >"
    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(TimeoutError):
        login_conta.confirmar(
            "conta-a", "CODE-123",
            estado_fake=lambda d: conta_estado._estado_login({"loggedIn": False}),
            timeout_s=0.3)

    assert bateia.digitadas[-1] == ("term-login-conta-a", "CODE-123")
    # O Enter foi enviado (o código precisa SUBMETER, não só ser digitado — B3).
    # Duas vezes: uma do comando (iniciar) e uma do código (confirmar).
    assert bateia.enters == ["term-login-conta-a", "term-login-conta-a"]
    # A janela só sai no fim (limpeza garantida em qualquer caminho) — e o alvo é o
    # REAL (term-<chave>), não a chave pedida (B2).
    assert bateia.matadas == ["term-login-conta-a"]


def test_confirmar_sem_tentativa_devolve_erro(bateia):
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123")

def test_confirmar_rele_o_estado_pelo_caminho_da_conta_nao_pelo_rotulo(bateia, monkeypatch):
    # B1 — a confirmação relê o estado pelo CAMINHO REAL da conta (dir_conta), nunca pelo
    # RÓTULO: com um rótulo relativo a CLI criava backend/<rotulo>/ dentro da árvore do
    # repo (medido: backend/conta-a/) e a conta nunca relia logada. O teste captura o
    # argumento que chega ao leitor real — o path, nunca o label.
    capturados = []

    def auth_status_captura(dir_conta):
        capturados.append(dir_conta)
        return {"loggedIn": True, "email": "u@exemplo.com", "subscriptionType": "max"}

    monkeypatch.setattr(conta_estado, "_auth_status", auth_status_captura)
    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u/.claude-conta-a")
    resultado = login_conta.confirmar("conta-a", "CODE-123")
    assert resultado["ok"] is True
    assert resultado["email"] == "u@exemplo.com"
    assert capturados == [Path("/home/u/.claude-conta-a")]


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
    # A janela morreu no fim do fluxo (caminho de sucesso) — pelo alvo REAL.
    assert bateia.matadas == ["term-login-conta-a"]


def test_confirmar_estado_indisponivel_nao_vira_logado(bateia):
    # `loggedIn: null` NÃO é deslogado: é formato que não dá pra confiar. A confirmação
    # recusa e a janela NÃO sobrevive.
    def estado_fake(dir_conta):
        return conta_estado._estado_login(None)   # cli-indisponivel

    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake)
    assert bateia.matadas == ["term-login-conta-a"]


def test_confirmar_timeout_deixa_janela_morta(bateia):
    def estado_fake(dir_conta):
        return conta_estado._estado_login({"loggedIn": False})

    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    with pytest.raises(TimeoutError):
        login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake, timeout_s=0.05)
    assert bateia.matadas == ["term-login-conta-a"]


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
    assert bateia.matadas == ["term-login-conta-a"]


def test_confirmar_antigo_nao_mata_janela_da_tentativa_nova(bateia):
    # B5 — corrida: o usuário cancela DURANTE a espera do confirmar e entra de novo. A
    # thread velha do confirmar não pode matar a janela da tentativa NOVA quando sair
    # (o `finally` limpa só o que é dela).
    def estado_fake(dir_conta):
        return conta_estado._estado_login({"loggedIn": False})

    def cancela_e_recomeca():
        time.sleep(0.02)
        login_conta.cancelar("conta-a")
        login_conta.iniciar("conta-a", "/home/u")

    monkeypatch_poll(0.001)
    login_conta.iniciar("conta-a", "/home/u")
    t = threading.Thread(target=cancela_e_recomeca)
    t.start()
    with pytest.raises(RuntimeError):
        login_conta.confirmar("conta-a", "CODE-123", estado_fake=estado_fake, timeout_s=5)
    t.join()
    # A tentativa nova CONTINUA em voo e a janela dela NÃO foi morta pela thread velha.
    # (O cancelar matou a primeira; a segunda segue viva.)
    assert login_conta._em_curso("conta-a")
    assert bateia.matadas == ["term-login-conta-a"]
    # Cancela a segunda (limpeza do teste).
    login_conta.cancelar("conta-a")
    assert bateia.matadas == ["term-login-conta-a", "term-login-conta-a"]


# -------------------------------------------------------------------------- helpers


def monkeypatch_poll(dt):
    # Reduz o sleep do poll de confirmação só nos testes que confirmam — a suíte não
    # espera o poll real de segundos.
    import app.login_conta as lc
    lc._POLL_S = dt
