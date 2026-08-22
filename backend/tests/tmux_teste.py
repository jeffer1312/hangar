"""Ajudantes para os testes que falam com o multiplexador DE VERDADE.

Existe por causa de um defeito medido no Windows em 22/08/2026: os teardowns matavam a sessao com
`kill-session -t "=<nome>"`, e o `=` e justamente o unico alvo que o psmux **nao** interpreta nesse
comando (`has-session`, `display`, `send-keys`, `new-window` e `split-window` aceitam; medido). O
resultado era o pior possivel:

    $ tmux kill-session -t "=zzKill"
    psmux: kill-session: session 'zzKill' still present after 5s     rc=1     real 5.2s
    $ tmux has-session -t zzKill                                     rc=0     (viva)

Cada limpeza pagava **5 segundos** de timeout e nao matava nada — e como o `rc` era engolido
(`capture_output=True` e mais nada), ninguem via. Encontrei **65** servidores `psmux ... -L
cp-test-<hash>` orfaos nesta maquina, e no `test_termsock` a sessao sobrevivente fazia o
`new-session` do caso SEGUINTE morrer com "duplicate session" (25 erros de setup).

Duas regras, e as duas importam:

1. o alvo sai de `tmux.alvo_de_kill`, a MESMA funcao que o codigo de producao usa — no POSIX
   continua `=<nome>` (match exato, que e o certo la e defende dos nomes que colidem por prefixo);
2. o teardown **falha alto** quando a sessao sobrevive. Limpeza silenciosa que nao limpa e como
   teste verde que nao testa: o estrago aparece longe, noutro caso, com outra cara.
"""
import subprocess

from app import tmux


def _base(sock: str | None) -> list[str]:
    return ["tmux", "-L", sock] if sock else ["tmux"]


def existe_sessao(nome: str, sock: str | None = None) -> bool:
    return subprocess.run([*_base(sock), "has-session", "-t", f"={nome}"],
                          capture_output=True).returncode == 0


def matar_sessao(nome: str, sock: str | None = None) -> None:
    """Mata a sessao e CONFERE que ela saiu. Nao existir ja e sucesso (idempotente, como o
    `tmux.kill_session`); sobreviver e AssertionError, porque a partir dai o proximo caso deste
    arquivo vai falhar por um motivo que nao e o dele."""
    cp = subprocess.run([*_base(sock), "kill-session", "-t", tmux.alvo_de_kill(nome)],
                        capture_output=True, text=True, errors="replace")
    if existe_sessao(nome, sock):
        raise AssertionError(
            f"sessao {nome!r} sobreviveu ao kill-session"
            + (f" (socket {sock})" if sock else "")
            + f": rc={cp.returncode} stderr={(cp.stderr or '').strip()!r}. "
            "Deixar passar vaza servidor do multiplexador e derruba o proximo caso com "
            "'duplicate session'."
        )
