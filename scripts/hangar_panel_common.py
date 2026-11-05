import shlex
import shutil
import subprocess
import sys


def comando_terminal(term: str, tmux_args: list, aviso: str) -> list:
    """ARGV do emulador que roda `tmux <tmux_args>` e, falhando, MOSTRA o motivo.

    Existe porque os dois cliques do painel que abrem terminal (hangar-panel-open, e o `project open`
    do hangar-panel-action) tinham a mesma falha silenciosa: `tmux attach` num alvo que já morreu sai
    em milissegundos e o terminal fecha antes de dar pra ler. O hangar-panel-open tapava isso com
    `new-session -A`, que CRIAVA um shell com o nome da sessão do agente — indistinguível dela na
    lista do app (medido 24/08/2026); o hangar-panel-action mandava o stderr pro DEVNULL e devolvia
    "log aberto no terminal" de qualquer jeito.

    O `|| { printf ...; read _; }` segura a janela aberta com a mensagem. `aviso` vai como
    ARGUMENTO do `%s`, nunca dentro do formato: nome de sessão com `%` viraria diretiva do printf.
    """
    cmd = " ".join(shlex.quote(a) for a in ["tmux", *tmux_args])
    return [term, "-e", "sh", "-c",
            f'{cmd} || {{ printf "\\n%s\\nEnter pra fechar. " {shlex.quote(aviso)}; read _; }}']


def avisar(msg: str) -> None:
    """Recusa que o usuário precisa ver quando NÃO há janela pra mostrá-la.

    O painel chama estes scripts com `execDetached` (shell.qml): ninguém lê o código de saída nem
    o stderr, então `print(..., file=sys.stderr)` sozinho é um clique que não faz nada. Notificação
    de desktop é o único canal que sobra quando nem terminal chegou a abrir. Segue imprimindo no
    stderr também — é o que aparece no log do quickshell pra quem for depurar.

    DEPENDÊNCIA: sem `notify-send` (e sem um daemon de notificação — mako, dunst, o do rice)
    sobra só o stderr, e aí o clique volta a não mostrar nada. Não há plano B sem inventar uma
    janela própria: quem usa o painel precisa dos dois instalados.
    """
    print(msg, file=sys.stderr)
    if shutil.which("notify-send"):
        subprocess.run(["notify-send", "-a", "hangar", "hangar", msg], check=False)
