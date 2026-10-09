"""Leitura de informacao de PROCESSO — a unica parte do backend que depende do /proc.

Todo o resto do app fala com processos so por estas funcoes. Elas estavam no registry.py,
misturadas com a logica de sessao; aqui ficam isoladas porque sao o UNICO ponto em que o
backend depende do sistema operacional ser Linux.

O /proc nao existe no Windows e tambem nao existe no macOS. Hoje as sete degradam em
silencio quando ele falta ({} vazio, "" ou None), e o efeito nao e "nao roda": e o app
perder o sinal AUTORITATIVO de qual .jsonl pertence a sessao (o --session-id da cmdline) e
cair no fallback newest-by-mtime, alem de nao enxergar CLAUDE_CONFIG_DIR nem CP_ENGINE de
uma sessao viva. Roda pior, sem avisar.

Sao DUAS implementacoes no MESMO namespace, de proposito — nao tres arquivos. Com um
`procinfo.py` importando de um `procinfo_proc.py`, um monkeypatch em `procinfo._proc_stat_path`
nao alcancaria o chamador la dentro, e o teste passaria por ACIDENTE lendo o /proc real. Um
arquivo, um namespace, um alvo de patch.
"""
import logging
import os
import shutil
import threading
import time
from pathlib import Path
from typing import Optional

_log = logging.getLogger("hangar.procinfo")

# Escolha da implementacao: por CAPACIDADE, uma vez, na importacao. Nao por nome de sistema —
# "e unix?" responde SIM pro macOS, que nao tem /proc, e mandaria o Mac ler /proc/<pid>/fd pra
# sempre falhar em silencio (era exatamente o que acontecia antes deste modulo existir).
#
# O `os.name == "posix"` na frente NAO desfaz isso: macOS e posix e continua caindo no ramo
# psutil pelo is_dir(), que e quem responde a pergunta de verdade. Ele existe porque no Windows
# "/proc" nem sequer e um caminho absoluto — e relativo ao DRIVE corrente, entao a pergunta que
# se estava fazendo era "existe C:\proc?" (ou D:\proc, conforme o cwd de quem subiu o backend).
# Medido em 21/08/2026: um teste criou C:\proc nesta VM e, a partir dali, TODA execucao pulava o
# `import psutil` e mandava o Windows pelo ramo /proc — onde cada leitura levanta OSError, e
# todas sao engolidas de proposito. Nao ha erro, nao ha log: cmdline vazio, mapa de filhos vazio,
# environ None. Na pratica o app perde provider do pane, --session-id, CLAUDE_CONFIG_DIR e
# CP_ENGINE de uma vez, e a sessao viva aparece como tracked=False.
_TEM_PROC = os.name == "posix" and Path("/proc").is_dir()

if not _TEM_PROC:
    # Importado SO fora do Linux. No Linux o psutil nem esta instalado (marcador
    # `sys_platform != 'linux'` no pyproject) e este import nunca roda.
    import psutil

# Por que o Linux nao migra pro psutil tambem, ja que ele funcionaria: `psutil.open_files()` e
# ordens de grandeza mais lento que listar /proc/<pid>/fd, e estes caminhos rodam por POLL, por
# sessao. Os comentarios do registry.py documentam otimizacoes feitas justamente pra derrubar
# forks por poll. Trocar codigo rapido que funciona por codigo portatil e mais lento seria uma
# regressao na plataforma que ja esta em producao.


# Cache do mapa de processos. O reuso DENTRO de uma listagem ja existia (o `children` passado
# adiante); o que faltava era reuso ENTRE chamadas. Medido: 38 rotas chamam `registry.list()` direto,
# sem passar pelo snapshot com TTL do api.py, e cada uma reconstruia o mapa — davam ~8 varreduras
# completas do /proc por segundo, metade de todo o trabalho do backend. O mapa e o mesmo pra todos os
# chamadores do mesmo instante. O TTL fica acima da cadencia do poll (1,5s): abaixo dela cada tick
# da lista achava o mapa vencido e varria de novo.
_MAPA_TTL = 3.0
# Caminho de ENVIO (agentpane.pane_info): logo apos criar a sessao o mapa de 3s pode nao conter o
# agente e a resolucao cairia no provider errado.
MAPA_TTL_ENVIO = 1.0
_mapa_cache: Optional[tuple[float, dict[int, list[int]]]] = None
_mapa_lock = threading.Lock()


def _invalidar_children_map() -> None:
    """Descarta o mapa cacheado. Para quem PRECISA ver um processo que acabou de nascer — sem isto o
    fallback de sessao recem-criada (api._guardar_snap com forcar) leria um mapa antigo e
    devolveria o mesmo 404 que ele existe pra evitar."""
    global _mapa_cache
    _mapa_cache = None


def _proc_children_map(max_age: float = _MAPA_TTL) -> dict[int, list[int]]:
    global _mapa_cache
    cache = _mapa_cache
    if cache is not None and time.monotonic() - cache[0] < max_age:
        return cache[1]
    with _mapa_lock:
        # Re-checa dentro do lock: sem isto N threads que erram juntas varrem o /proc N vezes, que e
        # exatamente o desperdicio que este cache existe pra tirar.
        cache = _mapa_cache
        if cache is not None and time.monotonic() - cache[0] < max_age:
            return cache[1]
        mapa = _varrer_children_map()
        _mapa_cache = (time.monotonic(), mapa)
        return mapa


def _varrer_children_map() -> dict[int, list[int]]:
    if not _TEM_PROC:
        return _children_map_psutil()
    # Mapa ppid->filhos varrendo o /proc/*/stat UMA vez. Caro (le o stat de todo processo da maquina);
    # por isso a listagem constroi UM mapa e reusa pra todas as sessoes (em vez de re-varrer por sessao).
    children: dict[int, list[int]] = {}
    try:
        entries = os.listdir("/proc")
    except OSError:
        return children
    for entry in entries:
        if not entry.isdigit():
            continue
        try:
            # ppid = 4o campo do stat; usar rsplit(')') pra nao quebrar com espaco/parenteses no comm.
            with open(f"/proc/{entry}/stat", encoding="utf-8", errors="replace") as fh:
                after = fh.read().rsplit(")", 1)[-1].split()
            ppid = int(after[1])
        except (OSError, ValueError, IndexError):
            continue
        children.setdefault(ppid, []).append(int(entry))
    return children


def _descendant_pids(root: int, children: Optional[dict[int, list[int]]] = None) -> list[int]:
    # root + todos os descendentes. O claude pode ser filho do shell do pane (sessao manual) ou o
    # proprio pane (app-criada com `claude` como comando). children: mapa pre-construido reusavel; se
    # None, constroi sob demanda (caminho single-session do SSE).
    if children is None:
        children = _proc_children_map()
    out, vistos, stack = [], set(), [root]
    while stack:
        p = stack.pop()
        if p in vistos:
            # ppid reciclado no Windows fecha anel no mapa (ppid aponta pra PID reaproveitado e o grafo deixa de ser arvore; /proc no Linux nunca fecha anel
            continue
        vistos.add(p)
        out.append(p)
        stack.extend(children.get(p, []))
    return out


def _open_jsonl(pid: int, projects_dir: Path) -> Optional[str]:
    if not _TEM_PROC:
        # Fora do Linux este sinal NAO vale o preco. `psutil.open_files()` no Windows enumera a
        # tabela de handles do sistema inteiro (NtQuerySystemInformation) — a propria doc do
        # psutil avisa que pode levar SEGUNDOS —, e isto roda por descendente, por sessao, a cada
        # list() (registry.py:431), num ciclo de 1,5s. O backend pararia.
        # O que se perde e pouco: o comentario abaixo ja registra que o claude nao segura o fd em
        # idle, e a medicao confirmou (9 descendentes, resultado None). A resolucao autoritativa
        # vem do --session-id do cmdline, que funciona igual nas duas plataformas.
        return None
    # 1o fd aberto apontando pra um *.jsonl dentro do projects_dir (= o transcript ativo do claude).
    # NOTA: o claude NAO segura esse fd em idle (abre/escreve/fecha) -> quase sempre None. Mantido so
    # como sinal extra confiavel QUANDO presente; a resolucao real vem do --session-id do cmdline.
    fddir = f"/proc/{pid}/fd"
    try:
        fds = os.listdir(fddir)
    except OSError:
        return None
    base = str(projects_dir)
    for fd in fds:
        try:
            target = os.readlink(f"{fddir}/{fd}")
        except OSError:
            continue
        if target.endswith(".jsonl") and target.startswith(base + os.sep):
            return target
    return None


def _cmdline(pid: int) -> str:
    if not _TEM_PROC:
        return _cmdline_psutil(pid)
    # cmdline crua do processo (args separados por NUL -> espaco).
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            return fh.read().replace(b"\x00", b" ").decode(errors="replace")
    except OSError:
        return ""


def _argv(pid: int) -> list[str]:
    """cmdline com as FRONTEIRAS preservadas — irmao do _cmdline, nao substituto dele.

    O `_cmdline` junta tudo com espaco (o lado /proc troca o NUL por espaco, e o psutil faz
    `" ".join` de proposito pra os dois sistemas darem o mesmo formato). Quem faz busca de
    substring continua usando ele. Mas quem precisa do argv0 nao pode reconstrui-lo com `.split()`:
    no Windows o caminho do executavel tem espaco na esmagadora maioria das instalacoes
    (`C:\\Program Files\\nodejs\\node.exe`), e o split devolve `C:\\Program` como argv0 — medido
    nesta VM, era o que fazia a deteccao de provider olhar pra "Program" e nao reconhecer nada. No
    Linux o mesmo codigo nunca falhou porque `/usr/bin/node` nao tem espaco; e a mesma armadilha
    que o comentario do _model_of ja descreve.
    """
    if not _TEM_PROC:
        try:
            return list(psutil.Process(pid).cmdline())
        except psutil.Error:
            return []
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            bruto = fh.read()
    except OSError:
        return []
    # O /proc termina a lista com um NUL, entao o ultimo pedaco vem vazio — descarta.
    return [a.decode("utf-8", "replace") for a in bruto.split(b"\x00") if a]


def _config_dir_of(pid: int) -> Optional[Path]:
    if not _TEM_PROC:
        v = _env_psutil(pid).get("CLAUDE_CONFIG_DIR")
        return Path(v) if v else None
    # CLAUDE_CONFIG_DIR do processo claude (setado pelo alias/picker). None se ausente -> fallback.
    try:
        with open(f"/proc/{pid}/environ", "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(b"CLAUDE_CONFIG_DIR="):
                    # surrogateescape = round-trip fiel dos bytes POSIX (a camada de fs do Python usa o
                    # mesmo) -> o Path ainda casa no disco mesmo com path nao-UTF-8. "replace" corromperia.
                    return Path(kv.split(b"=", 1)[1].decode("utf-8", "surrogateescape"))
    except OSError:
        return None
    return None


def _config_dir_of_strict(pid: int) -> tuple[Path | None, bool]:
    """A exclusão precisa distinguir variável ausente de ambiente ilegível."""
    if not _TEM_PROC:
        try:
            value = psutil.Process(pid).environ().get("CLAUDE_CONFIG_DIR")
        except psutil.Error:
            return None, False
        return (Path(value) if value else None), True
    try:
        with open(_proc_environ_path(pid), "rb") as fh:
            env = fh.read()
    except OSError:
        return None, False
    for entry in env.split(b"\x00"):
        if entry.startswith(b"CLAUDE_CONFIG_DIR="):
            value = entry.split(b"=", 1)[1].decode("utf-8", "surrogateescape")
            return (Path(value) if value else None), True
    return None, True


def _proc_environ_path(pid: int) -> str:
    # Indireção só para o teste poder apontar para um arquivo de mentira.
    return f"/proc/{pid}/environ"


def _proc_stat_path(pid: int) -> str:
    # Indireção só para o teste poder apontar para um arquivo de mentira (igual _proc_environ_path).
    return f"/proc/{pid}/stat"


# Raiz do /proc, indireta só para o teste da varredura apontar pra um /proc de mentira
# (mesma função dos _proc_environ_path/_proc_stat_path).
_PROC_ROOT = "/proc"


def taskkill_path() -> str | None:
    """O `taskkill.exe` do sistema, sem depender do PATH: a tarefa agendada que sobe o backend no
    Windows pode nascer sem o System32 nele. O PATH fica só de reserva."""
    exe = os.path.join(os.environ.get("SystemRoot") or r"C:\Windows", "System32", "taskkill.exe")
    return exe if os.path.isfile(exe) else shutil.which("taskkill")


def pid_vivo(pid: int) -> bool:
    """Aquele processo ainda existe? Pergunta que NAO pode ter efeito colateral.

    `os.kill(pid, 0)` responde isso no POSIX e no Windows faz outra coisa: qualquer sinal que nao
    seja CTRL_C_EVENT/CTRL_BREAK_EVENT vira `TerminateProcess` (o mesmo fato que `runner.py` ja
    anotava do outro lado, onde MATAR e a intencao). Medido em 26/08/2026 na maquina Windows de
    quem usa: o `estado_para_tela` da atualizacao chamava isso a cada poll da tela pra saber se o
    motor seguia vivo — e MATAVA o motor no meio da etapa de instalar, com o log congelado na
    ultima linha escrita e a tela dizendo "a atualizacao foi interrompida". A pergunta derrubava
    exatamente o que ela existia pra observar.
    """
    if pid <= 0:
        return False
    if not _TEM_PROC:
        return psutil.pid_exists(pid)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        # Vivo, e de outro dono. Os dois erros sao `OSError`, e o `except` unico que estava aqui
        # dizia "morto" pros dois — resposta que o ramo psutil ja dava certa, entao o mesmo pid
        # respondia diferente conforme o sistema. Dizer "morto" de um processo vivo aqui recolhe o
        # lock da atualizacao e larga dois `git reset --hard` no mesmo repo.
        return True
    return True


def _proc_start_time(pid: int) -> Optional[float]:
    if not _TEM_PROC:
        return _start_time_psutil(pid)
    # Instante (epoch, em segundos) em que o processo nasceu: campo 22 do /proc/<pid>/stat
    # (starttime, em ticks desde o boot) + o btime do /proc/stat. None = não deu pra ler (pid morto,
    # permissão, kernel sem /proc) -> quem chama decide, aqui só degrada como os vizinhos.
    try:
        with open(_proc_stat_path(pid)) as fh:
            raw = fh.read()
        # O comm (campo 2) vem entre parênteses e pode conter espaço E parêntese ("(pi (2))"), então
        # contar campo por espaço a partir do início erra. O último ')' é o único delimitador
        # confiável: depois dele o campo 22 é o 20º token.
        ticks = float(raw[raw.rindex(")") + 1:].split()[19])
        with open("/proc/stat") as fh:
            for line in fh:
                if line.startswith("btime "):
                    return float(line.split()[1]) + ticks / os.sysconf("SC_CLK_TCK")
    except (OSError, ValueError, IndexError):
        return None
    return None


def _engine_of(pid: int) -> Optional[str]:
    if not _TEM_PROC:
        return _env_psutil(pid).get("CP_ENGINE") or None
    # Motor de modelo do processo claude (CP_ENGINE, injetado por engines.env_de via hangar-engine
    # --exec). None = conta Anthropic. Mesmo truque do _config_dir_of: o env do processo VIVO é o
    # registro autoritativo — sidecar em disco pode divergir do que está rodando no pane.
    try:
        with open(_proc_environ_path(pid), "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(b"CP_ENGINE="):
                    return kv.split(b"=", 1)[1].decode("utf-8", "replace") or None
    except OSError:
        return None
    return None


def _model_of(pid: int) -> tuple[str | None, str | None]:
    """Modelo e esforço com que a sessão SUBIU, lidos do cmdline.

    Par do _engine_of, que lê o motor do environ pelo mesmo motivo: sem isto, retomar pelo app monta
    `claude --resume <sid>` pelado e a sessão volta pro modelo do motor — a escolha some sem aviso.
    """
    # `.split()` obrigatório: _cmdline devolve STRING (a versão /proc troca os NUL por espaço,
    # e a psutil faz `" ".join` de propósito pra os dois sistemas darem o mesmo formato).
    # Sem o split, `argv.index("--model")` seria índice de CARACTERE e `argv[i+1]` devolveria uma
    # letra — o resume montaria `--model -`, calado. E NÃO mexa no _cmdline: os dois consumidores
    # atuais (agentpane.py:48-51 e _session_id_from_cmdline) fazem operação de string.
    argv = _cmdline(pid).split()
    if not argv:
        return None, None

    def _val(flag: str) -> str | None:
        if flag in argv:
            i = argv.index(flag)
            if i + 1 < len(argv):
                return argv[i + 1]
        return None

    return _val("--model"), _val("--effort") or _val("--thinking")


def _env_var_of(pid: int, nome: str) -> str | None:
    """Uma variável do environ do processo, irmã do _engine_of (que lê só CP_ENGINE).

    O resume remonta o prefixo hangar-engine com a janela da sessão que está morrendo
    (CLAUDE_CODE_MAX_CONTEXT_TOKENS): sem ler do processo vivo, a sessão ressuscitaria com a flag
    num modelo e o ambiente noutro — o cenário "motor de 1M com modelo de 262k" depois de um
    resume e sem nada na tela acusando.
    """
    if not _TEM_PROC:
        return _env_psutil(pid).get(nome) or None
    try:
        with open(_proc_environ_path(pid), "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(f"{nome}=".encode()):
                    return kv.split(b"=", 1)[1].decode("utf-8", "replace") or None
    except OSError:
        return None
    return None


# ─────────────────────────────────────────────────────────────────────────────────────────────
# Implementacao psutil — Windows e macOS. Contrato IDENTICO ao de cima, degradacao inclusive:
# processo morto / sem permissao devolve {} , "" ou None, nunca excecao. `psutil.Error` cobre
# NoSuchProcess, AccessDenied e ZombieProcess de uma vez; deixar qualquer uma escapar viraria
# 500 no meio de um poll de listagem so porque uma sessao morreu entre duas leituras.
# ─────────────────────────────────────────────────────────────────────────────────────────────


def _children_map_psutil() -> dict[int, list[int]]:
    children: dict[int, list[int]] = {}
    try:
        # attrs= faz UMA varredura trazendo so pid/ppid, em vez de um Process por pid e duas
        # chamadas cada — mesma intencao do "varre /proc UMA vez" do lado Linux.
        for proc in psutil.process_iter(attrs=["pid", "ppid"]):
            info = proc.info
            children.setdefault(info["ppid"], []).append(info["pid"])
    except psutil.Error:
        return children
    return children


def _cmdline_psutil(pid: int) -> str:
    # Junta com espaco igual ao lado Linux (que troca o NUL por espaco), pra o
    # _session_id_from_cmdline receber exatamente o mesmo formato nas duas plataformas.
    try:
        return " ".join(psutil.Process(pid).cmdline())
    except psutil.Error:
        return ""


def _env_psutil(pid: int) -> dict[str, str]:
    # Um so leitor de ambiente pros dois consumidores (CLAUDE_CONFIG_DIR e CP_ENGINE). Do lado
    # /proc eles duplicam a leitura por razao historica; aqui nao ha motivo pra repetir.
    try:
        return psutil.Process(pid).environ() or {}
    except psutil.Error:
        return {}


def _start_time_psutil(pid: int) -> Optional[float]:
    # create_time() ja vem em epoch/segundos — nada de ticks nem btime, e nenhum parse de comm
    # com parenteses. E a unica das seis em que a versao portatil e mais simples que a nativa.
    try:
        return psutil.Process(pid).create_time()
    except psutil.Error:
        return None


# O Claude Code roda cada Bash num shell que carrega um prologo (snapshot do ambiente, guardas de
# glob) antes do comando de verdade, que vai dentro de um `eval '...'`. Mostrar a linha inteira
# enche a tela de prologo; o que interessa e o que foi pedido.
# Ancorado no `&& eval '` do PROLOGO (a primeira ocorrencia: o prologo vem antes do comando). Sem a
# ancora, um comando cujo TEXTO contenha `eval '` era cortado no meio.
# O fim e lido como string de shell (aspa simples fecha; `'"'"'` e `'\''` sao aspa escapada), nao
# pela ultima aspa da linha: no Windows com %TEMP% curto (ADMINI~1) o sufixo vem entre aspas
# (`pwd -P >| '/c/Users/ADMINI~1/...'`) e o corte guloso levava ele junto.
_EVAL_INICIO = "&& eval '"
_ASPA_ESCAPADA = ("""'"'"'""", "'\\''")

# Filhos que NAO sao comando do agente: servidor MCP em stdio, hook e a propria statusline nascem
# como filhos diretos do processo e apareciam no chip como "comando ainda rodando" — a statusline,
# viva desde o inicio da sessao, lia como um travamento de horas. O comando do agente sempre passa
# por um shell, entao o shell e o criterio.
_SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "fish"}


def _comando_pedido(bruto: str) -> str:
    i = bruto.find(_EVAL_INICIO)
    if i < 0:
        return bruto.strip()
    j = i + len(_EVAL_INICIO)
    partes: list[str] = []
    while True:
        k = bruto.find("'", j)
        if k < 0:
            partes.append(bruto[j:])
            break
        partes.append(bruto[:k][j:])
        esc = next((e for e in _ASPA_ESCAPADA if bruto.startswith(e, k)), None)
        if esc is None:
            break
        partes.append("'")
        j = k + len(esc)
    return "".join(partes).strip()


def shells_de(pid: int) -> list[dict]:
    """Comandos de shell que esta sessao deixou rodando: os filhos DIRETOS do processo do agente.

    Existe porque "a sessao esta ocupada" e "sobrou um comando rodando" sao coisas diferentes, e so
    a segunda explica por que uma sessao parada aparece trabalhando. Filho direto, nao descendente:
    o que interessa e o comando que o agente disparou, nao a arvore que ele abriu.

    Degrada como o resto do modulo: sem /proc e sem psutil, lista vazia.
    """
    try:
        filhos = _proc_children_map().get(pid, [])
    except Exception:  # noqa: BLE001 - leitura de processo nunca derruba quem pergunta
        # Lista vazia aqui e indistinguivel de "nao ha comando rodando", e a sessao aparece limpa
        # justamente quando a TUI diz que ha shell de pe: o log e o unico jeito de saber depois.
        _log.warning("shells_de(%s): nao consegui ler os filhos do processo", pid, exc_info=True)
        return []
    out: list[dict] = []
    for f in filhos:
        if not _e_shell(f):
            continue
        cmd = _comando_pedido(_cmdline(f))
        if not cmd:
            continue
        out.append({"pid": f, "cmd": cmd, "desde": _proc_start_time(f)})
    return out


def _e_saida_de_tarefa(caminho: str) -> bool:
    # `<tmp>/claude-<uid>/…/tasks/<id>.output` no Linux, `%TEMP%\claude\…\tasks\<id>.output` no Windows.
    p = Path(caminho)
    return p.suffix == ".output" and p.parent.name == "tasks" and any(
        parte.startswith("claude") for parte in p.parts)


_COMANDOS_TTL = 2.0
_comandos_cache: Optional[tuple[float, list[tuple["psutil.Process", str]]]] = None
_comandos_lock = threading.Lock()


def _comandos_vivos() -> list[tuple["psutil.Process", str]]:
    """(processo, comando) de todos os processos, varridos uma vez por janela: cada card de Bash
    aberto pergunta a cada 2 s, e sem isto cada pergunta varria a máquina inteira."""
    global _comandos_cache
    with _comandos_lock:
        cache = _comandos_cache
        if cache is not None and time.monotonic() - cache[0] < _COMANDOS_TTL:
            return cache[1]
        vivos = []
        for proc in psutil.process_iter(attrs=["pid", "cmdline"]):
            argv = proc.info.get("cmdline")
            if argv:
                vivos.append((proc, _comando_pedido(" ".join(argv))))
        _comandos_cache = (time.monotonic(), vivos)
        return vivos


def _arquivo_da_saida(alvo: str) -> Optional[str]:
    if not _TEM_PROC:
        # `open_files()` só roda no processo que casou (medido na VM Windows: 20-40 ms), nunca na
        # lista inteira.
        try:
            for proc, comando in _comandos_vivos():
                if comando != alvo:
                    continue
                try:
                    for f in proc.open_files():
                        if _e_saida_de_tarefa(f.path):
                            return f.path
                except psutil.Error:
                    continue
        except psutil.Error:
            return None
        return None
    try:
        entradas = os.listdir(_PROC_ROOT)
    except OSError:
        return None
    for entrada in entradas:
        if not entrada.isdigit():
            continue
        try:
            destino = os.readlink(f"{_PROC_ROOT}/{entrada}/fd/1")
        except OSError:
            continue
        if _e_saida_de_tarefa(destino) and _comando_pedido(_cmdline(int(entrada))) == alvo:
            return destino
    return None


def saida_de_comando(comando: str, teto: int = 16_000) -> Optional[str]:
    """Últimos bytes da saída de um Bash do Claude Code ainda rodando; None se não está rodando.

    O Claude Code manda a saída do comando para `tasks/<id>.output` e apaga o arquivo no fim. O
    nome não traz o id da chamada: quem liga os dois é o processo, que escreve no arquivo e traz o
    comando no `eval '...'` da própria linha de comando.
    """
    alvo = comando.strip()
    if not alvo:
        return None
    arquivo = _arquivo_da_saida(alvo)
    if not arquivo:
        return None
    try:
        with open(arquivo, "rb") as f:
            f.seek(max(0, os.fstat(f.fileno()).st_size - teto))
            return f.read().decode("utf-8", "replace")
    except OSError:
        return None


def _e_shell(pid: int) -> bool:
    # argv0 vem do `_argv`, nao de split no `_cmdline`: o caminho do executavel tem espaco na
    # maioria das instalacoes Windows e o split devolveria "C:\\Program" como argv0 — a mesma
    # armadilha que o docstring do `_argv` ja registra, medida naquela VM.
    argv = _argv(pid)
    return bool(argv) and os.path.basename(argv[0]) in _SHELLS
