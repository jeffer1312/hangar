"""Janela escondida do login Claude, operada pelo Rust (`ClaudeWindows` em `account_bridge`).

O fluxo de Entrar numa conta é do Rust: ele decide e confirma o login relendo o estado da conta.
Daqui sai só a I/O com o tmux por trás de `/internal/accounts/claude-window`: abrir a janela
escondida com o `CLAUDE_CONFIG_DIR` da conta, digitar o comando, ler o pane (o link de
autorização e o prompt do código), entregar o código por stdin e matar a janela.
"""
import re
import uuid

from app import tmux

# O CLI do Claude imprime a URL OAuth como hiperlink OSC 8 no pane (medido em 17/08). A
# regex casa o TEXTO da URL (a segunda ocorrência do par OSC 8), não o rótulo.
_URL_RE = re.compile(r"(https?://[^\s\x1b]+)")
# O prompt em que o CLI espera o código colado (medido em 17/08).
_PROMPT_RE = re.compile(r"Paste code here if prompted", re.I)


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

def _shell_code(nome: str, codigo: str) -> None:
    """Entrega o código por stdin, sem fallback que o exponha em argv."""
    if not codigo or len(codigo) > 4096 or any(c in codigo for c in ("\n", "\r", "\x00")):
        raise ValueError("código inválido")
    buffer = "hangar-login-" + uuid.uuid4().hex
    try:
        loaded = tmux._run(["tmux", "load-buffer", "-b", buffer, "-"], input=codigo.encode())
        if loaded.returncode != 0:
            raise RuntimeError("não consegui entregar o código à janela de login")
        pasted = tmux._run(["tmux", "paste-buffer", "-t", tmux._pane_target(nome), "-b", buffer, "-d"])
        if pasted.returncode != 0 or not tmux.send_keys(nome, "Enter"):
            raise RuntimeError("não consegui confirmar o código na janela de login")
    finally:
        tmux._run(["tmux", "delete-buffer", "-b", buffer])


def _shell_ler(nome: str) -> str:
    """I/O: lê o pane da janela escondida (tmux.capture_pane), JUNTANDO linhas quebradas.

    `juntar=True`: o CLI quebra a URL OAuth na margem de 80 colunas da janela escondida e
    o capture cru devolveria a quebra como \n — o link tocável da tela morreria no meio
    (B2). O padrão da primitiva continua False: TODO o resto do backend lê desenho de TUI
    linha a linha.
    """
    return tmux.capture_pane(nome, juntar=True)


def _shell_matar(nome: str) -> None:
    """Só confirma a limpeza depois de provar que a janela saiu."""
    if not tmux.kill_session(nome):
        raise RuntimeError("a janela de login continua aberta")
    probe = tmux._run(["tmux", "has-session", "-t", "=" + nome])
    if probe.returncode != 1:
        raise RuntimeError("não consegui confirmar o fim da janela de login")
