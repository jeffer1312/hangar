"""Login numa conta pelo app (Task 7) — a costura por cima das primitivas que já existem.

O botão Entrar da aba Contas (ContasSettings.svelte) chama este módulo; o fluxo completo:

1. `iniciar` — abre UMA janela escondida por tentativa (`tmux.new_hidden_shell`, a mesma
   sessão `term-*` do painel de terminal) e digita `claude auth login --claudeai`. O CLI
   imprime a URL OAuth no pane e espera "Paste code here if prompted >".
2. `passo` — lê o pane e devolve o endereço de autorização (link tocável na tela) e a
   etapa em que o fluxo está.
3. `confirmar` — digita o código colado e CONFIRMA **relendo o estado da conta**
   (`conta_estado._estado_login`, a MESMA fonte da Task 4) até a conta aparecer logada.
   NUNCA pela aparência da tela: o CLI não imprime confirmação textual estável, e
   "aparece logada" é o único sinal verdadeiro de que a autorização completou.
4. `cancelar` — mata a janela. Em NENHUM caminho (sucesso, erro, timeout, cancelamento
   durante a espera) a janela escondida sobrevive: o dono da tentativa é o módulo, e a
   limpeza é garantida.

Uma janela por tentativa, por conta (chave `login-<nome>` — o MESMO espaço de nomes das
sessões escondidas do painel de terminal). Tentativa em voo recusa começar de novo.

A I/O com o tmux é só nas quatro funções privadas `_shell_*` abaixo, trocadas nos testes
por texto de mentira (precedente: `engine_probe._buscar`, `conta_estado._auth_status`).
"""
import itertools
import logging
import re
import time
from dataclasses import dataclass

from app import conta_estado, tmux

_log = logging.getLogger("claude_pocket.login_conta")

# O CLI do Claude imprime a URL OAuth como hiperlink OSC 8 no pane (medido em 17/08). A
# regex casa o TEXTO da URL (a segunda ocorrência do par OSC 8), não o rótulo.
_URL_RE = re.compile(r"(https?://[^\s\x1b]+)")
# O prompt em que o CLI espera o código colado (medido em 17/08).
_PROMPT_RE = re.compile(r"Paste code here if prompted", re.I)
# Espera da confirmação: o OAuth demora segundos pra propagar o token até a conta. O
# usuário vê o passo 4 ("confirmação") enquanto isso; 5 minutos é folga confortável e o
# cancelamento interrompe a espera a qualquer momento.
_TIMEOUT_S = 300.0
_POLL_S = 0.5

_PREFIXO = "login-"


def _chave_janela(conta: str) -> str:
    return f"{_PREFIXO}{conta}"


def _shell_criar(nome: str, cwd: str, config_dir: str | None = None) -> str | None:
    """I/O: cria a janela escondida (tmux.new_hidden_shell). None = tmux recusou.

    `config_dir` (o path da conta) vai pro `-e CLAUDE_CONFIG_DIR` do new-session:
    o `claude auth login` grava o `.credentials.json` no config dir do AMBIENTE do
    pane, nao no cwd — sem isto a credencial iria pra conta ativa do servidor tmux
    (B4).
    """
    return tmux.new_hidden_shell(nome, cwd, config_dir=config_dir)


def _shell_digitar(nome: str, texto: str) -> None:
    """I/O: digita texto no pane da janela escondida (terminal_input.send_text)."""
    from app import terminal_input
    terminal_input.TerminalInput().send_text(nome, texto)


def _shell_submeter(nome: str, texto: str) -> None:
    """I/O: digita o texto e ENVIA Enter — sem isto o comando/código fica digitado e
    nunca executa (B3; o precedente é `TerminalInput.select`, que manda o Enter
    explicitamente depois de navegar).

    Recebe o ALVO real (`term-<chave>`), não a chave — o send-keys/capture-pane
    acertam a sessão que existe de verdade (B2).
    """
    _shell_digitar(nome, texto)
    if not tmux.send_keys(nome, "Enter"):
        raise RuntimeError(f"nao consegui enviar Enter para a janela de login de {nome}")


def _shell_ler(nome: str) -> str:
    """I/O: lê o pane da janela escondida (tmux.capture_pane)."""
    return tmux.capture_pane(nome)


def _shell_matar(nome: str) -> None:
    """I/O: mata a janela escondida (tmux.kill_session)."""
    tmux.kill_session(nome)


# A tentativa em voo por conta: identidade (id que so cresce) + alvo REAL da janela (o
# retorno de `new_hidden_shell`, que e `term-<chave>` e NAO a chave pedida — B2) + relógio.
# Nao é persistido de propósito: um backend reiniciado não deixa janela pendurada (a
# limpeza é por processo).
@dataclass
class Tentativa:
    id: int
    alvo: str
    inicio: float

_tentativas: dict[str, Tentativa] = {}
_proximo_id = itertools.count(1)


def _em_curso(conta: str) -> bool:
    """Há uma tentativa de login em voo pra esta conta?"""
    return conta in _tentativas


def _limpar(conta: str, t: Tentativa | None = None) -> None:
    """Remove o registro da tentativa e garante a janela morta. Idempotente.

    `t` é a identidade: quando a chamada pertence a uma tentativa específica
    (confirmar), só limpa se AINDA for a mesma — uma tentativa antiga nunca pode
    matar a janela de uma nova (B5: corrida cancelar→Entrar).
    """
    if t is None:
        _tentativas.pop(conta, None)
        try:
            _shell_matar(_chave_janela(conta))
        except Exception:
            _log.debug("login: matar janela de %s falhou", conta, exc_info=True)
        return
    if _tentativas.get(conta) is not t:
        return
    _tentativas.pop(conta, None)
    try:
        _shell_matar(t.alvo)
    except Exception:
        # Matar janela que não existe é sucesso (kill_session é idempotente); falha de
        # tmux aqui não pode mascarar o resultado do fluxo.
        _log.debug("login: matar janela de %s falhou", conta, exc_info=True)


def iniciar(conta: str, cwd: str) -> dict:
    """Abre a janela escondida e digita o comando de login. Recusa se já há uma tentativa."""
    if _em_curso(conta):
        raise RuntimeError(f"login já em andamento para a conta {conta}")
    chave = _chave_janela(conta)
    alvo = _shell_criar(chave, cwd, config_dir=cwd)
    if alvo is None:
        raise RuntimeError(f"não consegui abrir a janela escondida para {conta}")
    # Registra ANTES de digitar: um erro de digitação cai no caminho de erro e a limpeza
    # sabe qual janela matar.
    tentativa = Tentativa(id=next(_proximo_id), alvo=alvo, inicio=time.monotonic())
    _tentativas[conta] = tentativa
    try:
        _shell_submeter(alvo, "claude auth login --claudeai")
    except Exception:
        _limpar(conta, tentativa)
        raise
    return {"ok": True}


def passo(conta: str) -> dict:
    """Etapa atual do fluxo, lida do pane: a URL de autorização (quando já apareceu).

    - "idle": nenhuma tentativa em voo
    - "aguardando": janela aberta; `url` presente quando o CLI já imprimiu o endereço
    """
    if not _em_curso(conta):
        return {"etapa": "idle", "url": None}
    t = _tentativas[conta]
    texto = _shell_ler(t.alvo)
    m_url = _URL_RE.search(texto)
    url = m_url.group(1) if m_url else None
    if url and _PROMPT_RE.search(texto):
        return {"etapa": "aguardando", "url": url}
    return {"etapa": "aguardando", "url": url}


def confirmar(conta: str, codigo: str, *, estado_fake=None, timeout_s: float = _TIMEOUT_S) -> dict:
    """Digita o código e espera a conta reler logada. Devolve o e-mail e o plano.

    A confirmação é por RELEITURA do estado da conta (`conta_estado._estado_login`, a
    mesma fonte da Task 4), nunca pela aparência da tela. `estado_fake` é a porta de
    teste (mesma regra do `_auth_status`); sem ele, lê o estado real da conta.

    Nunca levanta por falha de leitura: estado `indisponivel` é recusa com erro claro.
    No fim, em QUALQUER caminho, a janela morre.
    """
    if not _em_curso(conta):
        raise RuntimeError(f"nenhuma tentativa de login em voo para a conta {conta}")
    tentativa = _tentativas[conta]
    try:
        _shell_submeter(tentativa.alvo, codigo)
    except Exception:
        _limpar(conta, tentativa)
        raise

    ler_estado = estado_fake or (lambda d: conta_estado._estado_login(
        conta_estado._auth_status(d)))
    inicio = time.monotonic()
    try:
        while True:
            estado = ler_estado(conta)
            if estado.estado == "ok" and estado.loggedIn:
                return {
                    "ok": True,
                    "email": estado.email,
                    "plano": estado.plano,
                }
            if _tentativas.get(conta) is not tentativa:
                # O usuário cancelou durante a espera e JÁ COMEÇOU OUTRA tentativa: a
                # janela é de outra pessoa agora — sai sem tocar em nada (B5).
                raise RuntimeError(f"login da conta {conta} cancelado")
            if estado.estado != "ok":
                raise RuntimeError(f"não consegui reler o estado da conta {conta}: "
                                   f"{estado.motivo or 'indisponivel'}")
            if time.monotonic() - inicio >= timeout_s:
                raise TimeoutError(f"a conta {conta} não apareceu logada em {timeout_s:.0f}s")
            time.sleep(_POLL_S)
    finally:
        # Já cancelada: `_limpar` não remata (a janela morreu no cancelar). A identidade
        # garante que uma tentativa velha nunca mata a janela de uma nova (B5).
        _limpar(conta, tentativa)


def cancelar(conta: str) -> dict:
    """Cancela a tentativa em voo e mata a janela escondida. No-op sem tentativa."""
    if not _em_curso(conta):
        return {"ok": True}
    _limpar(conta, _tentativas[conta])
    return {"ok": True}
