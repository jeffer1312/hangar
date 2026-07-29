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
import os
from pathlib import Path
from typing import Optional

# Escolha da implementacao: por CAPACIDADE, uma vez, na importacao. Nao por nome de sistema —
# "e unix?" responde SIM pro macOS, que nao tem /proc, e mandaria o Mac ler /proc/<pid>/fd pra
# sempre falhar em silencio (era exatamente o que acontecia antes deste modulo existir).
_TEM_PROC = Path("/proc").is_dir()

if not _TEM_PROC:
    # Importado SO fora do Linux. No Linux o psutil nem esta instalado (marcador
    # `sys_platform != 'linux'` no pyproject) e este import nunca roda.
    import psutil

# Por que o Linux nao migra pro psutil tambem, ja que ele funcionaria: `psutil.open_files()` e
# ordens de grandeza mais lento que listar /proc/<pid>/fd, e estes caminhos rodam por POLL, por
# sessao. Os comentarios do registry.py documentam otimizacoes feitas justamente pra derrubar
# forks por poll. Trocar codigo rapido que funciona por codigo portatil e mais lento seria uma
# regressao na plataforma que ja esta em producao.


def _proc_children_map() -> dict[int, list[int]]:
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
    out, stack = [], [root]
    while stack:
        p = stack.pop()
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


def _proc_environ_path(pid: int) -> str:
    # Indireção só para o teste poder apontar para um arquivo de mentira.
    return f"/proc/{pid}/environ"


def _proc_stat_path(pid: int) -> str:
    # Indireção só para o teste poder apontar para um arquivo de mentira (igual _proc_environ_path).
    return f"/proc/{pid}/stat"


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
    # Motor de modelo do processo claude (CP_ENGINE, injetado por engines.env_de via cp-engine
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
