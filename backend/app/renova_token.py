"""Janela escondida da renovação de token Claude, operada pelo Rust (`ClaudeWindows`).

Quem decide renovar a conta parada é o Rust; daqui sai só a I/O com o tmux por trás de
`/internal/accounts/claude-window`: achar uma pasta confiada da conta e abrir nela, com o
`CLAUDE_CONFIG_DIR` da conta, uma janela escondida que roda `claude`.

Abrir `claude` numa pasta NÃO confiada trava na pergunta "Is this a project you created or one you
trust?" — por isso `pasta_confiada` lê as pastas já aceitas daquela conta e a conta sem nenhuma é
pulada, nunca aberta no escuro.
"""
import json
from pathlib import Path

from app import tmux


def pasta_confiada(dir_conta: Path) -> Path | None:
    """Primeira pasta confiada e AINDA EXISTENTE daquela conta, ou None.

    Confiança é por conta e por caminho: mora no `<config>/.claude.json` dela, em
    `projects[<caminho>].hasTrustDialogAccepted`. O `is True` é literal — um `"true"` string ou um
    `1` não é a resposta que o CLI grava, e tratar como sim colocaria a sessão exatamente no prompt
    de confiança que este módulo existe pra evitar.

    `is_dir()` porque o histórico guarda projeto que já foi apagado: nascer num diretório que sumiu
    é o CLI reclamando no lugar de renovar.
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


def _criar_janela(nome: str, cwd: str, dir_conta: str) -> str | None:
    """Janela escondida no cwd da pasta confiada, com o `CLAUDE_CONFIG_DIR` da conta.

    O config dir vai por `tmux new-session -e` porque é CAMINHO, não segredo — a regra dura do repo
    (chave nunca por linha de comando nem por `-e`, senão ela aterrissa no `/proc/<pid>/cmdline`,
    legível pela máquina inteira) não é violada aqui. E ele é obrigatório: sem o `-e`, o pane herda
    o ambiente do SERVIDOR tmux e o `claude` renovaria a conta errada, em silêncio.

    `new_hidden_shell` e não `new_session` por causa da marca `@cp_hidden`: sem ela o registry trata
    o pane como sessão Claude e a renovação vira um CARD nas três views do app, aparecendo e sumindo
    sozinho.
    """
    return tmux.new_hidden_shell(nome, cwd, config_dir=dir_conta)


def _submeter(alvo: str, comando: str) -> None:
    """Digita o comando e manda o Enter. Sem o Enter o `claude` fica escrito no pane e nunca sobe —
    e a espera abaixo estouraria o timeout inteiro achando que o CLI é lento."""
    if not tmux.send_keys(alvo, comando, literal=True):
        raise RuntimeError(f"não consegui digitar na janela de renovação {alvo}")
    if not tmux.send_keys(alvo, "Enter"):
        raise RuntimeError(f"não consegui enviar Enter para a janela de renovação {alvo}")


def _matar(alvo: str) -> None:
    tmux.kill_session(alvo)
