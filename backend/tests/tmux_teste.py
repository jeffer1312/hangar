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
import time
import uuid

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


# ---------------------------------------------------------------------------
# O SERVIDOR, nao so a sessao (medido no psmux 3.3.7, 23/08/2026)
#
# Matar a ultima sessao do socket NAO encerra o servidor aqui. O psmux mantem um processo
# `tmux server -s __warm__ -L <socket>` de pe (pane pre-aquecido), e ele nunca e recolhido:
#
#     $ tmux -L cp-probe-c21 new-session -d -s zz ...
#     $ tmux -L cp-probe-c21 kill-session -t zz          rc=0   (a sessao morre mesmo)
#     $ tmux -L cp-probe-c21 list-sessions               rc=0   ''      <- parece limpo
#     $ ps                                               tmux.exe server -s __warm__ -L cp-probe-c21
#
# `list-sessions` responde IGUAL num socket virgem e num socket com o `__warm__` vivo (rc=0 e
# saida vazia nos dois), entao nao ha como perguntar isso ao multiplexador — quem responde e a
# tabela de processos. Cada arquivo de teste que cria um socket proprio deixava um desses pra
# tras: 70 orfaos nesta VM em 22/08/2026, ~12,7 GB de working set (cada um segura um
# `powershell` e um `conhost`), e a sessao Claude que rodava a suite morreu com a maquina no
# teto de memoria (0xc00000fd, pilha estourada).
#
# No Linux o mesmo defeito e inofensivo — o servidor sai sozinho com a ultima sessao e sobra o
# arquivo de socket, 0 byte. O conserto e o mesmo nos dois: `kill-server` NO SOCKET PROPRIO.
# ---------------------------------------------------------------------------

# Sockets entregues por `novo_socket`. O conjunto e o que permite a suite PROVAR, no fim, que nao
# sobrou servidor de teste nenhum (ver o fixture `_sem_servidor_de_teste_vazado` no conftest).
_SOCKETS: set[str] = set()


def novo_socket(prefixo: str = "cp-test") -> str:
    """Nome de socket unico pra este teste, JA registrado pra conferencia no fim da suite.

    Fabricar o nome na mao (`f"cp-test-{uuid4().hex[:8]}"`, como era) funciona igual — o que se
    perde e a lista, e sem a lista o vazamento so aparece quando a maquina acaba.
    """
    return _registrar(f"{prefixo}-{uuid.uuid4().hex[:8]}")


def _registrar(sock: str) -> str:
    _SOCKETS.add(sock)
    return sock


def processos_do_socket(sock: str) -> list[tuple[int, str]]:
    """(pid, cmdline) de todo processo que ainda cita este socket.

    E a UNICA pergunta que responde "o servidor saiu?" — `list-sessions` nao distingue socket
    virgem de socket com servidor vazio (medido; ver o bloco acima). O nome tem uuid, entao um
    casamento por substring nao pega processo alheio.
    """
    import psutil
    achados = []
    for p in psutil.process_iter(["cmdline"]):
        try:
            linha = " ".join(p.info["cmdline"] or ())
        except (psutil.Error, TypeError):    # processo morreu no meio da varredura
            continue
        if sock in linha:
            achados.append((p.pid, linha))
    return achados


def matar_servidor(sock: str) -> None:
    """Recolhe o servidor DESTE socket e CONFERE que nao sobrou processo — mesma regra do
    `matar_sessao`: limpeza silenciosa que nao limpa e como teste verde que nao testa.

    `sock` vazio levanta. `kill-server` sem `-L` derruba o servidor tmux DEFAULT e com ele todas as
    sessoes de trabalho de quem roda a suite — e a proibicao que os quatro teardowns ja carregavam
    escrita; aqui ela vira codigo, num lugar so.
    """
    if not sock:
        raise ValueError("matar_servidor exige o socket: `kill-server` sem `-L` derruba o servidor "
                         "tmux DEFAULT (todas as sessoes do usuario), nao o socket deste teste")
    cp = subprocess.run(["tmux", "-L", sock, "kill-server"],
                        capture_output=True, text=True, errors="replace")
    # Idempotente e barato nos dois: 0,1s no psmux, inclusive num socket que nunca existiu (medido).
    # O `rc` nao serve de prova — o psmux responde 0 tendo matado servidor ou nao tendo achado
    # nenhum —, entao quem confirma e a tabela de processos, com folga pro processo sair.
    sobra = processos_do_socket(sock)
    for _ in range(10):
        if not sobra:
            break
        time.sleep(0.1)
        sobra = processos_do_socket(sock)
    _SOCKETS.discard(sock)
    if sobra:
        raise AssertionError(
            f"servidor do socket {sock!r} sobreviveu ao kill-server: rc={cp.returncode} "
            f"stderr={(cp.stderr or '').strip()!r}, processos={sobra}. Cada um destes segura um "
            "shell e um console vivos ate a maquina reiniciar."
        )


def sockets_vazados() -> dict[str, list[tuple[int, str]]]:
    """Sockets entregues por `novo_socket` que AINDA tem processo vivo. Vazio = suite limpa."""
    return {s: p for s in sorted(_SOCKETS) if (p := processos_do_socket(s))}
