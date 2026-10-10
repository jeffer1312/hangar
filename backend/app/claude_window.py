"""Janela escondida do Claude que o Rust opera por `/internal/accounts/claude-window` (`ClaudeWindows`).

O login e a renovação de token das contas Claude são do Rust; daqui sai só a I/O com o tmux: abrir
a janela com o `CLAUDE_CONFIG_DIR` da conta, digitar o comando, ler o pane (o link de autorização e
o prompt do código), entregar o código por stdin e matar a janela.
"""
import json
import re
import uuid
from pathlib import Path

from app import tmux

# O CLI do Claude imprime a URL OAuth como hiperlink OSC 8 no pane (medido em 17/08). A
# regex casa o TEXTO da URL (a segunda ocorrência do par OSC 8), não o rótulo.
URL_RE = re.compile(r"(https?://[^\s\x1b]+)")
# O prompt em que o CLI espera o código colado (medido em 17/08).
PROMPT_RE = re.compile(r"Paste code here if prompted", re.IGNORECASE)


def trusted_folder(dir_conta: Path) -> Path | None:
    """Primeira pasta confiada e AINDA EXISTENTE da conta, onde a renovação abre o `claude`.

    Abrir numa pasta NÃO confiada trava na pergunta "Is this a project you created or one you
    trust?". A confiança mora no `<config>/.claude.json` da conta, em
    `projects[<caminho>].hasTrustDialogAccepted`, e o `is True` é literal: é o que o CLI grava.
    `is_dir()` porque o histórico guarda projeto que já foi apagado.
    """
    try:
        dados = json.loads((dir_conta / ".claude.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    projetos = dados.get("projects") if isinstance(dados, dict) else None
    if not isinstance(projetos, dict):
        return None
    for caminho, cfg in projetos.items():
        if not isinstance(cfg, dict) or cfg.get("hasTrustDialogAccepted") is not True:
            continue
        p = Path(caminho)
        if p.is_dir():
            return p
    return None


def spawn(nome: str, cwd: str, config_dir: str) -> str | None:
    """Cria a janela escondida e devolve o alvo real (`term-<nome>`); None = tmux recusou.

    O config dir da conta vai no `-e CLAUDE_CONFIG_DIR`: a CLI grava e renova a credencial no
    config dir do AMBIENTE do pane, e sem ele mexeria na conta ativa do servidor tmux. É caminho,
    não segredo. `new_hidden_shell` marca o pane como escondido: sem isso ele viraria card de sessão.
    """
    return tmux.new_hidden_shell(nome, cwd, config_dir=config_dir)


def submit(alvo: str, comando: str) -> None:
    """Digita o comando e manda o Enter: sem ele o comando fica escrito no pane e nunca roda."""
    if not tmux.send_keys(alvo, comando, literal=True):
        raise RuntimeError(f"não consegui digitar na janela escondida {alvo}")
    if not tmux.send_keys(alvo, "Enter"):
        raise RuntimeError(f"não consegui enviar Enter para a janela escondida {alvo}")


def send_code(alvo: str, codigo: str) -> None:
    """Entrega o código por stdin, sem fallback que o exponha em argv."""
    if not codigo or len(codigo) > 4096 or any(c in codigo for c in ("\n", "\r", "\x00")):
        raise ValueError("código inválido")
    buffer = "hangar-login-" + uuid.uuid4().hex
    try:
        loaded = tmux._run(["tmux", "load-buffer", "-b", buffer, "-"], input=codigo.encode())
        if loaded.returncode != 0:
            raise RuntimeError("não consegui entregar o código à janela de login")
        pasted = tmux._run(["tmux", "paste-buffer", "-t", tmux._pane_target(alvo), "-b", buffer, "-d"])
        if pasted.returncode != 0 or not tmux.send_keys(alvo, "Enter"):
            raise RuntimeError("não consegui confirmar o código na janela de login")
    finally:
        tmux._run(["tmux", "delete-buffer", "-b", buffer])


def read(alvo: str) -> str:
    """Lê o pane JUNTANDO as linhas quebradas: a CLI quebra a URL OAuth na margem de 80 colunas,
    e o capture cru cortaria o link no meio."""
    return tmux.capture_pane(alvo, juntar=True)


def kill(alvo: str) -> None:
    """Só confirma a limpeza depois de provar que a janela saiu."""
    if not tmux.kill_session(alvo):
        raise RuntimeError("a janela escondida continua aberta")
    probe = tmux._run(["tmux", "has-session", "-t", "=" + alvo])
    if probe.returncode != 1:
        raise RuntimeError("não consegui confirmar o fim da janela escondida")
