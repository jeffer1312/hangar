"""Launcher de projetos dev — standalone, NAO atrelado a sessao Claude viva.

Config em backend/projects.json ({nome: {cwd, command, port?, stop_command?}}), gitignored
porque os caminhos sao desta maquina; molde em projects.json.example. Roda por cima do
runner.py (mesmo socket tmux dedicado, 1 run por cwd), entao play/stop/log funcionam igual
ao runner por-sessao — a diferenca e so a chave: nome de projeto do config, nao sessao viva.

Estados: stopped (sem sessao tmux) / starting (pane vivo, porta configurada ainda fechada) /
running (pane vivo e porta aberta, ou sem porta configurada) / failed (pane morto via
remain-on-exit — o log final fica capturavel ate o proximo play/stop).
"""
import json
import logging
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from app import atomico, runner
from app.models import ProjectStatus, RunInfo
from app.procinfo import _TEM_PROC

_log = logging.getLogger("claude_pocket.projects")

if os.name == "nt":
    import msvcrt
else:
    import fcntl


def _trava_exclusiva(fh) -> None:
    """Lock EXCLUSIVO e BLOQUEANTE no arquivo aberto. Solto ao fechar, nos dois sistemas.

    Import no topo do modulo era `import fcntl` puro — POSIX only. Como o api.py importa este
    modulo, isso derrubava o backend inteiro na SUBIDA no Windows, com ImportError: nao era
    degradacao de funcionalidade, era o servidor nao nascer.

    LK_LOCK trava 1 byte e, se outro processo ja segura, tenta por ~10s e ai levanta OSError —
    barulhento de proposito. O caso que este lock existe pra impedir (import disparando varios
    POST concorrentes, o ultimo a gravar apagando as entries dos outros) falha em SILENCIO; um
    erro alto e melhor que a corrupcao calada.
    """
    if os.name == "nt":
        msvcrt.locking(fh.fileno(), msvcrt.LK_LOCK, 1)
    else:
        fcntl.flock(fh, fcntl.LOCK_EX)

_CONFIG = Path(__file__).resolve().parent.parent / "projects.json"
_STOP_TIMEOUT = 30


class ProjectError(Exception):
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status, self.detail = status, detail


def _load() -> dict:
    try:
        data = json.loads(_CONFIG.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as e:
        # Config quebrado vira erro VISIVEL no painel (via errors[]), nunca lista vazia muda.
        raise ProjectError(500, f"projects.json invalido: {e}") from e
    return data if isinstance(data, dict) else {}


def _validate(name: str, cwd: str, command: str, port: object) -> None:
    """Barra entrada ruim ANTES de gravar (400 com motivo claro). name é chave de arquivo, então
    sem '/' nem '..' (path traversal); cwd tem que existir; command não-vazio."""
    if not name or "/" in name or ".." in name:
        raise ProjectError(400, "nome inválido (sem '/' nem '..')")
    if not isinstance(command, str) or not command.strip():
        raise ProjectError(400, "command é obrigatório")
    if not Path(os.path.expanduser(cwd)).is_dir():
        raise ProjectError(400, f"cwd não existe: {cwd}")
    if port is not None and not isinstance(port, int):
        raise ProjectError(400, "port deve ser inteiro")


def _mutate(fn) -> None:
    """Read-modify-write do projects.json inteiro sob lock EXCLUSIVO, atômico (mkstemp 0600 +
    os.replace). Espelha cp_panel_common.set_peer_enabled: o import dispara vários POST
    concorrentes, e sem lock quem grava por último apaga as entries dos outros — sem erro, sem log.
    `fn(data)` muta o dict in place."""
    lock_path = _CONFIG.with_name(_CONFIG.name + ".lock")
    lock = open(lock_path, "w")
    with lock:
        _trava_exclusiva(lock)
        for stale in _CONFIG.parent.glob(_CONFIG.name + ".*.tmp"):
            stale.unlink(missing_ok=True)
        data = _load()      # FileNotFound -> {}; JSON inválido -> ProjectError(500)
        fn(data)
        fd, tmp_name = tempfile.mkstemp(dir=_CONFIG.parent, prefix=_CONFIG.name + ".", suffix=".tmp")
        tmp = Path(tmp_name)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
            if _CONFIG.exists():
                os.chmod(tmp, _CONFIG.stat().st_mode & 0o777)
            atomico.substituir(tmp, _CONFIG)
        except OSError as e:
            tmp.unlink(missing_ok=True)
            # `atomico.explicar`, nao `{e}` cru: esta e das poucas mensagens de escrita que chegam
            # INTEIRAS na tela (a rota faz `HTTPException(e.status, e.detail)` com a string). No
            # Windows o rename por cima do projects.json aberto por outro processo levanta
            # PermissionError, e "Acesso negado" manda a pessoa conferir o ACL de um arquivo que
            # ela pode escrever.
            raise ProjectError(500,
                               f"falha ao gravar projects.json: {atomico.explicar(e)}") from e


def upsert(name: str, cwd: str, command: str, port: int | None = None,
           stop_command: str | None = None) -> "ProjectStatus":
    """Cria ou MESCLA a entry `name`: campos não passados (port/stop_command/futuros) são
    preservados — editar só a porta não zera o stop_command feito na mão."""
    _validate(name, cwd, command, port)

    def mut(data: dict) -> None:
        entry = data.get(name) if isinstance(data.get(name), dict) else {}
        entry["cwd"] = cwd
        entry["command"] = command
        if port is not None:
            entry["port"] = port
        if stop_command is not None:
            entry["stop_command"] = stop_command
        data[name] = entry

    _mutate(mut)
    cfg = _entry(name)
    return _status(name, cfg, runner.all_runs(), _ports_of([(name, cfg)]))


def remove(name: str) -> None:
    """Tira a entry do projects.json. Recusa (409) se houver run vivo — deletar por baixo de um
    processo rodando deixaria órfão invisível (sem port não vira nem 'external'). Pare antes."""
    cfg = _load().get(name)
    if not isinstance(cfg, dict):
        raise ProjectError(404, f"projeto '{name}' não está no projects.json")
    info = runner.all_runs().get(runner._slug(str(cfg.get("cwd", ""))))
    if info is not None and not info.exited:
        raise ProjectError(409, f"projeto '{name}' está rodando — pare antes de remover")

    def mut(data: dict) -> None:
        data.pop(name, None)

    _mutate(mut)


def _entry(name: str) -> dict:
    cfg = _load().get(name)
    if not isinstance(cfg, dict):
        raise ProjectError(404, f"projeto '{name}' nao esta no projects.json")
    if not isinstance(cfg.get("cwd"), str) or not isinstance(cfg.get("command"), str):
        raise ProjectError(500, f"projects.json: '{name}' precisa de cwd e command (string)")
    return cfg


# Dono da porta que NAO deu pra identificar — diferente de "o dono e outro projeto". Comeca com
# '<', que nenhum cwd real comeca, entao nunca colide com um caminho. Existe porque os dois casos
# tinham virado o mesmo None, e um pane VIVO com o servidor no ar ficava "starting" pra sempre.
# Acontece no macOS, onde psutil.net_connections() exige root.
DONO_INDETERMINADO = "<indeterminado>"


def _port_info(ports: set[int]) -> dict[int, tuple[bool, str | None]]:
    """porta -> (escutando?, cwd realpath do processo dono do LISTEN).

    Dono None = ninguem escuta ou nao ha o que atribuir; DONO_INDETERMINADO = alguem escuta mas
    o sistema nao deixou ver quem.

    O dono importa: porta 3000 aberta e QUALQUER front — sem conferir o cwd de quem segura a
    porta, todo projeto configurado com a mesma porta apareceria "rodando" junto. Uma varredura
    so de /proc/net/tcp* e /proc/*/fd para TODAS as portas: o custo e por poll, nao por projeto.
    """
    out: dict[int, tuple[bool, str | None]] = {p: (False, None) for p in ports}
    if not ports:
        return out
    if not _TEM_PROC:
        return _port_info_psutil(ports, out)
    want = {f"{p:04X}": p for p in ports}
    inodes: dict[str, int] = {}  # socket:[ino] -> porta
    for path in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            lines = Path(path).read_text().splitlines()[1:]
        except OSError:
            continue
        for ln in lines:
            f = ln.split()
            # st 0A = LISTEN; campo 1 = addr:porta em hex; campo 9 = inode do socket.
            if len(f) > 9 and f[3] == "0A":
                hexport = f[1].rsplit(":", 1)[-1]
                if hexport in want:
                    port = want[hexport]
                    out[port] = (True, None)
                    inodes[f"socket:[{f[9]}]"] = port
    if not inodes:
        return out
    pending = set(inodes.values())
    for pid in os.listdir("/proc"):
        if not pending:
            break
        if not pid.isdigit():
            continue
        try:
            fds = os.scandir(f"/proc/{pid}/fd")
        except OSError:
            continue  # processo de outro usuario/ja morto: dono fica None, nunca atribuido
        for fd in fds:
            try:
                port = inodes.get(os.readlink(fd.path))
            except OSError:
                continue
            if port is not None and port in pending:
                try:
                    out[port] = (True, os.path.realpath(f"/proc/{pid}/cwd"))
                except OSError:
                    pass
                pending.discard(port)
    return out


def _port_info_psutil(ports: set[int], out: dict[int, tuple[bool, str | None]]
                      ) -> dict[int, tuple[bool, str | None]]:
    """Mesma resposta do caminho /proc, via psutil — Windows e macOS.

    Windows NAO precisa de elevacao aqui: net_connections() enxerga os processos do proprio
    usuario, e os dev servers sao dele. macOS precisa de root, e a saida NAO e pedir isso: o
    backend segura o CP_AUTH_TOKEN e cria sessoes, subir tudo como root por causa de "que porta
    esta aberta" e trocar um incomodo por um risco. Sem permissao, cai no probe de socket, que
    responde SE alguem escuta sem dizer QUEM — e o dono vira DONO_INDETERMINADO.
    """
    import psutil   # so fora do Linux; no Linux nem esta instalado (marcador no pyproject)
    try:
        for c in psutil.net_connections(kind="inet"):
            if c.status != psutil.CONN_LISTEN or not c.laddr or c.laddr.port not in ports:
                continue
            dono: str | None = DONO_INDETERMINADO
            if c.pid:
                try:
                    dono = psutil.Process(c.pid).cwd()
                except psutil.Error:
                    pass          # processo morreu entre a varredura e agora, ou sem permissao
            out[c.laddr.port] = (True, dono)
    except psutil.Error:
        # macOS sem root: nem a varredura sai. Sobra saber se ALGUEM escuta.
        for p in ports:
            out[p] = (_alguem_escuta(p), DONO_INDETERMINADO if _alguem_escuta(p) else None)
    return out


def _alguem_escuta(port: int) -> bool:
    # Sem privilegio nenhum: se o connect completa, tem alguem aceitando. Loopback so — a
    # pergunta e sobre dev server local, e varrer outra interface seria varredura de rede.
    import socket
    with socket.socket() as s:
        s.settimeout(0.15)
        return s.connect_ex(("127.0.0.1", port)) == 0


def _owns(owner: str | None, cwd: str) -> bool:
    """Dono da porta e ESTE projeto? cwd igual ou subpasta (PSS sobe de deploy/)."""
    if not owner or owner == DONO_INDETERMINADO:
        return False
    root = os.path.realpath(cwd)
    return owner == root or owner.startswith(root + os.sep)


def _status(name: str, cfg: dict, runs: dict[str, RunInfo],
            ports: dict[int, tuple[bool, str | None]]) -> ProjectStatus:
    cwd = str(cfg.get("cwd", ""))
    port = cfg.get("port") if isinstance(cfg.get("port"), int) else None
    listening, owner = ports.get(port, (False, None)) if port else (False, None)
    # Porta so conta pro projeto se o processo que a segura estiver NO cwd dele (ou subpasta):
    # 3000 e default de meio mundo de front — sem checar o dono, todo projeto com a mesma porta
    # configurada apareceria "rodando" junto.
    mine = listening and _owns(owner, cwd)
    slug = runner._slug(cwd)
    info = runs.get(slug)
    if info is None:
        # Porta de pe, dono dentro do projeto, sem pane nosso = rodando FORA do launcher
        # (subido na mao/por outra sessao). Dono em outra pasta ou nao identificavel: NAO e
        # dele — fica "stopped", sem atribuicao falsa.
        state = "external" if mine else "stopped"
    elif info.exited:
        state = "failed"
    elif port and not mine and listening and owner == DONO_INDETERMINADO:
        # Porta de pe, dono NAO identificavel (macOS sem root) e um pane NOSSO vivo neste cwd.
        # Aqui "starting" seria mentira definida — o servidor esta servindo. Atribuir ao pane e
        # o palpite muito mais provavel: fomos nos que subimos algo neste cwd e a porta abriu.
        # Nao vale pro caso SEM pane, que segue "stopped": ali nao ha nada nosso pra creditar.
        state = "running"
    elif port and not mine:
        # Pane vivo mas a porta ainda nao e dele (fechada, ou aberta por OUTRO projeto —
        # nesse caso o dev server vai morrer de EADDRINUSE e o card vira "failed" com log).
        state = "starting"
    else:
        state = "running"
    stop_cmd = cfg.get("stop_command")
    return ProjectStatus(name=name, cwd=cwd, command=str(cfg.get("command", "")), port=port,
                         state=state, since=info.since if info else None,
                         exit_status=info.exit_status if info else None, tmux=slug,
                         has_stop_command=isinstance(stop_cmd, str) and bool(stop_cmd.strip()))


# Falhou primeiro (pede ação), depois vivos, parados por último — o painel abre mostrando o
# que importa sem scroll. starting e running com o mesmo peso: a linha não pula quando a
# porta abre.
_ORDER = {"failed": 0, "running": 1, "starting": 1, "external": 1}


def _ports_of(entries: list[tuple[str, dict]]) -> dict[int, tuple[bool, str | None]]:
    return _port_info({c["port"] for _, c in entries if isinstance(c.get("port"), int)})


def list_projects() -> list[ProjectStatus]:
    runs = runner.all_runs()
    entries = [(n, c) for n, c in _load().items() if isinstance(c, dict)]
    ports = _ports_of(entries)
    out = [_status(n, c, runs, ports) for n, c in entries]
    out.sort(key=lambda p: (_ORDER.get(p.state, 2), p.name.lower()))
    return out


def start(name: str) -> ProjectStatus:
    cfg = _entry(name)
    if not Path(cfg["cwd"]).is_dir():
        raise ProjectError(400, f"cwd nao existe: {cfg['cwd']}")
    runner.start_run(cfg["cwd"], cfg["command"])
    return _status(name, cfg, runner.all_runs(), _ports_of([(name, cfg)]))


def _primeiro_token(comando: str) -> str:
    """O que o shell vai tentar EXECUTAR — o resto da linha nao interessa aqui.

    Aspas primeiro por causa de `"C:\\Program Files\\app\\stop.exe" --tudo`: cortar no espaco ali
    daria `"C:\\Program`, e o diagnostico acusaria um comando que ninguem escreveu.
    """
    c = comando.strip()
    if c.startswith('"'):
        return c[1:].partition('"')[0]
    return c.split()[0] if c.split() else ""


# Palavras que o cmd.exe executa SOZINHO — nao ha arquivo pra achar no PATH, e procurar por elas
# acusaria de "nao existe" um `cd ... && taskkill ...`, que e stop_command legitimo no Windows.
_BUILTINS_CMD = frozenset("""assoc break call cd chdir cls color copy date del dir echo endlocal
erase exit for ftype goto if md mkdir mklink move path pause popd prompt pushd rd rem ren rename
rmdir set setlocal shift start time title type ver verify vol""".split())


def _stop_nao_rodou(stop_cmd: str, cwd: str, r: subprocess.CompletedProcess) -> str:
    """Mensagem quando o `stop_command` saiu != 0 no Windows — ou "" pra seguir calado.

    `rc != 0` sozinho NAO e falha, e nao virou: `pkill` devolve 1 quando ja nao ha o que matar, e
    o equivalente daqui, `taskkill /IM x`, devolve **128** ("o processo nao foi encontrado" —
    medido). Acusar por rc faria toda parada de projeto ja parado virar erro na tela.

    O que a revisao apontou e outra coisa: um `stop_command` com sintaxe POSIX (comum quando o
    projeto veio de uma maquina Linux) o cmd.exe nem chega a rodar — `pkill -f 'node server.js'`
    da rc=1 dizendo que nao reconhece o comando —, o pane morre do mesmo jeito, a UI diz "parado"
    e o processo de verdade fica orfao. Foi exatamente o cenario que o comentario do `stop_run`
    diz querer evitar.

    Separar os dois pelo rc nao da (1 e 128 sao os dois lados), e pelo stderr menos ainda: as duas
    mensagens do cmd.exe vem traduzidas pro idioma do Windows. O que separa, sem depender de
    idioma, e se o comando EXISTE: `taskkill` esta no PATH, `pkill` nao. Por isso a checagem so
    roda depois de um rc != 0 — comando que funcionou nao paga nada, e um `pkill x || taskkill y`
    que deu certo continua calado.

    `cwd` entra na busca porque o cmd.exe procura no diretorio ATUAL antes do PATH, e o
    subprocess roda com `cwd` no projeto: sem isso, um `stop.bat` ao lado do codigo seria acusado
    de inexistente. O `shutil.which` aplica o PATHEXT, entao `stop` acha `stop.bat`.
    """
    nome = _primeiro_token(stop_cmd)
    if not nome or nome.lower() in _BUILTINS_CMD:
        return ""
    caminho = os.environ.get("PATH", "") + os.pathsep + str(cwd)
    if shutil.which(nome, path=caminho) is not None:
        # Existe e falhou: pode ser o "nao havia o que matar" de sempre. Nao acusa, mas deixa
        # rastro — e o unico jeito de entender depois um `kill $(cat pid)`, que EXISTE aqui (vem
        # do Git) e falha por sintaxe.
        _log.info("stop_command de %s saiu rc=%s: %s", cwd, r.returncode,
                  _saida_curta(r.stderr))
        return ""
    return (f"stop_command nao rodou: o Windows nao tem `{nome}` (nem no PATH nem em {cwd}) — "
            f"o processo que ele mataria pode ter ficado orfao")


def _saida_curta(bruto: bytes | str | None) -> str:
    """Cauda do stderr pro LOG. Nunca pra tela: o cmd.exe responde na codepage OEM do console
    (cp850 nesta maquina, medido), que nao e a do `locale` — decodificar errado aqui daria uma
    mensagem torta pro usuario em cima de um diagnostico que ja e sobre bytes."""
    if not bruto:
        return ""
    if isinstance(bruto, bytes):
        bruto = bruto.decode(_CODEPAGE_OEM, errors="replace")
    return " | ".join(bruto.split())[:200]


def _codepage_oem() -> str:
    """Codepage do console, pra decodificar o que o cmd.exe escreve. Fora do Windows nao e usada."""
    if os.name != "nt":
        return "utf-8"
    try:
        import ctypes
        return f"cp{ctypes.windll.kernel32.GetOEMCP()}"
    except Exception:                                    # noqa: BLE001
        return "cp850"


_CODEPAGE_OEM = _codepage_oem()


def stop(name: str) -> None:
    cfg = _entry(name)
    stop_cmd = cfg.get("stop_command")
    err = ""
    if isinstance(stop_cmd, str) and stop_cmd.strip():
        # Projeto que fabrica processos FORA do pane (PSS: 18 modulos em background) precisa do
        # proprio stop — matar so o pane deixaria os filhos orfaos rodando. Exit != 0 nao e
        # falha: pkill devolve 1 quando ja nao ha processo pra matar.
        try:
            # `/bin/sh` chumbado aqui nao existe no Windows — o stop_command do projeto NUNCA
            # rodava la, e a mensagem generica de falha nao dizia por que. `runner.argv_de_shell`
            # e o mesmo lugar que o start usa (POSIX byte-identico; Windows vai por COMSPEC).
            r = subprocess.run(runner.argv_de_shell(stop_cmd), cwd=cfg["cwd"],
                               capture_output=True, timeout=_STOP_TIMEOUT)
            if os.name == "nt" and r.returncode != 0:
                err = _stop_nao_rodou(stop_cmd, cfg["cwd"], r)
        except subprocess.TimeoutExpired:
            err = f"stop_command estourou {_STOP_TIMEOUT}s — confira processos orfaos"
        except OSError as e:
            err = f"stop_command falhou: {e}"
    # O pane morre SEMPRE, mesmo com stop_command quebrado — senao o projeto ficava "rodando"
    # eterno. Mas a falha do stop_command sobe depois: orfao invisivel e pior que erro na tela.
    runner.stop_run(cfg["cwd"])
    if err:
        raise ProjectError(500, err)


def pane(name: str) -> str:
    return runner.run_pane(_entry(name)["cwd"])
