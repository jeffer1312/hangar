import hashlib
import json
import logging
import os
import re
import shlex
import signal
import subprocess
import time
import tomllib
from pathlib import Path
from typing import Optional

from app import atomico
from app.config import settings
from app.models import Runner, RunInfo
from app.tmux import _scope_prefix, alvo_de_kill

# O `start_command`/`stop_command` do projeto e uma LINHA DE SHELL escrita pelo usuario ("cd x &&
# npm run dev"), entao ela precisa de um shell — nao da pra mandar como argv. As duas funcoes
# abaixo dizem QUAL shell, e existem porque o caminho POSIX nao roda no Windows: medido nesta VM,
# `exec {SHELL} -lc <cmd>` no pane do psmux NAO executa nada (nem citando o SHELL, que aqui e o
# bash do Git e tem espaco no caminho), e o `/bin/sh` que o stop chamava simplesmente nao existe
# (FileNotFoundError [WinError 2]). Medido tambem o que FUNCIONA no Windows: `cmd /c <linha>`
# executa, no pane e por subprocess, com `&&` e tudo.
#
# `COMSPEC` e o espelho exato do `SHELL` do POSIX (e o que o proprio `subprocess(shell=True)` usa
# no Windows), entao os dois ramos tem a mesma forma: variavel de ambiente com um padrao.
# O ramo POSIX sai BYTE-IDENTICO ao de antes — mesma string, mesmo `exec`, mesmo `shlex.quote`.
def _linha_de_shell_no_pane(command: str) -> str:
    """A string que vai pro `new-session` pra rodar `command` como linha de shell."""
    if os.name == "nt":
        # Sem `shlex.quote`: ele e citacao POSIX e o `cmd /c` toma o RESTO da linha como comando —
        # citar transformaria a linha do usuario em um argumento literal.
        return f'{os.environ.get("COMSPEC", "cmd.exe")} /c {command}'
    shell = os.environ.get("SHELL", "/bin/sh")
    # login shell (-lc) herda env/PATH do projeto; exec faz o comando virar dono do pane.
    return f"exec {shell} -lc {shlex.quote(command)}"


def argv_de_shell(command: str) -> list[str]:
    """argv pra rodar `command` como linha de shell por `subprocess` (e o caminho do stop)."""
    if os.name == "nt":
        return [os.environ.get("COMSPEC", "cmd.exe"), "/c", command]
    return ["/bin/sh", "-lc", command]


# nome -> peso pra escolher o melhor palpite de "dev" (so um vence).
_DEV_RANK = {"dev": 5, "start": 4, "serve": 3, "watch": 2, "run": 1}
_MAKE_TARGET = re.compile(r"^([a-zA-Z0-9_-]+):", re.MULTILINE)


def _pm(cwd: Path) -> str:
    # package manager pelo lockfile; default npm.
    if (cwd / "pnpm-lock.yaml").is_file():
        return "pnpm"
    if (cwd / "bun.lockb").is_file():
        return "bun"
    if (cwd / "yarn.lock").is_file():
        return "yarn"
    return "npm"


def _scan_package_json(cwd: Path) -> list[Runner]:
    try:
        data = json.loads((cwd / "package.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    scripts = data.get("scripts") if isinstance(data, dict) else None
    if not isinstance(scripts, dict):
        return []
    pm = _pm(cwd)
    out = []
    for name in scripts:
        if isinstance(name, str) and name:
            out.append(Runner(label=name, command=f"{pm} run {name}", source="npm"))
    return out


def _scan_makefile(cwd: Path) -> list[Runner]:
    try:
        text = (cwd / "Makefile").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    seen, out = set(), []
    for m in _MAKE_TARGET.finditer(text):
        t = m.group(1)
        if t not in seen:
            seen.add(t)
            out.append(Runner(label=t, command=f"make {t}", source="make"))
    return out


def _scan_stack(cwd: Path) -> list[Runner]:
    out = []
    if (cwd / "Cargo.toml").is_file():
        out.append(Runner(label="cargo run", command="cargo run", source="stack"))
    pyproj = cwd / "pyproject.toml"
    if pyproj.is_file():
        try:
            d = tomllib.loads(pyproj.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            d = {}
        project = d.get("project") if isinstance(d, dict) else None
        scripts = project.get("scripts") if isinstance(project, dict) else None
        if isinstance(scripts, dict):
            for name in scripts:
                if isinstance(name, str) and name:
                    out.append(Runner(label=name, command=f"uv run {name}", source="stack"))
    return out


def detect_runners(cwd: str) -> list[Runner]:
    """Comandos de execucao detectados no projeto. Tolerante a arquivos ausentes/malformados."""
    base = Path(cwd)
    runners = _scan_package_json(base) + _scan_makefile(base) + _scan_stack(base)
    best_i, best_score = -1, 0
    for i, r in enumerate(runners):
        score = _DEV_RANK.get(r.label.lower(), 0)
        if score > best_score:
            best_i, best_score = i, score
    if best_i >= 0:
        runners[best_i].is_dev_guess = True
    return runners


def _prefs_path() -> Path:
    return Path(settings.projects_dir).parent / ".claude-pocket-runner.json"


def _load_prefs() -> dict:
    try:
        data = json.loads(_prefs_path().read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def remembered(cwd: str) -> Optional[str]:
    v = _load_prefs().get(cwd)
    return v if isinstance(v, str) else None


def remember(cwd: str, command: str) -> None:
    p = _prefs_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    d = _load_prefs()
    d[cwd] = command
    tmp = p.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(d), encoding="utf-8")
    atomico.substituir(tmp, p)  # escrita atomica


_log = logging.getLogger(__name__)

RUN = subprocess.run
SOCK = "cppkt-run"  # socket tmux dedicado -> nao aparece na lista de sessoes do app


class RunnerError(Exception):
    """Falha do play/stop que a TELA precisa ver.

    Existe porque o modo antigo de errar aqui era o pior possivel: o `new-session` rodava dentro de
    um `except: pass`, com o `rc` nem olhado, entao "nao subiu" e "subiu" terminavam iguais — e o
    `run_status` logo abaixo devolvia o estado da sessao VELHA como se fosse a nova. E a mesma
    regra que o `projects.stop` ja escreve pro `stop_command`: orfao invisivel e pior que erro na
    tela.

    `detail` e texto cru, como o `ProjectError` do painel de projetos, que e por onde estes erros
    chegam na tela.
    """

    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status, self.detail = status, detail


def _sock(args: list[str]) -> subprocess.CompletedProcess:
    try:
        return RUN(["tmux", "-L", SOCK, *args], capture_output=True, text=True,
                   encoding="utf-8", errors="replace", timeout=5)
    except (subprocess.TimeoutExpired, OSError) as e:
        return subprocess.CompletedProcess(args, 1, "", str(e))


def _existe(name: str) -> bool:
    """`has-session` NESTE socket. O `=` (match exato) e honrado por `has-session` nos dois
    multiplexadores — e a excecao deliberada do psmux, que o recusa em `kill-session`."""
    return _sock(["has-session", "-t", f"={name}"]).returncode == 0


def _matar_run(name: str) -> bool:
    """True = a sessao do run NAO existe mais depois desta chamada (inclui "ja nao existia").

    O mesmo verificar-DEPOIS do `tmux.kill_session`, replicado aqui porque este socket e proprio
    (`cppkt-run`) e nao passa por la. Sem ele, "o comando deu 0" era lido como "a sessao saiu", e
    as duas coisas nao sao a mesma: medido nesta VM (psmux 3.3.7, 23/08/2026), com um run de
    verdade no pane,

        kill-session -t "=proj-igual"   rc=1  5,1s  "session ... still present after 5s"   VIVA
        new-session  -s proj-igual      rc=1        "duplicate session: proj-igual"
        list-sessions                               proj-igual: 1 windows    <- a VELHA

    ou seja: aperta "reiniciar", o processo antigo continua rodando, o novo nunca sobe, e a tela
    mostra a sessao velha como se fosse a nova. Sem um erro em lugar nenhum.

    O ALVO ja estava certo e continua vindo do `alvo_de_kill`: no Windows ele devolve o nome CRU,
    que e exatamente o que este arquivo sempre mandou — e e o unico que funciona, como a medida
    acima mostra (com o `=`, o psmux espera 5s e nao mata). Trocar isso por um `=` fixo, "pra
    ficar igual ao resto", INTRODUZIRIA o defeito aqui. No POSIX ele acrescenta o `=`, que e a
    defesa contra o resolve por PREFIXO — la o comportamento so muda no caso ja quebrado (nome
    ausente + irma por prefixo), como a docstring do proprio `alvo_de_kill` registra.

    Sobreviveu: mata o processo do pane direto, o mesmo contorno que o instalador ja usa (a sessao
    e NOSSA e o `pane_pid` sai daqui) — nunca `kill-server`, que levaria junto os runs dos outros
    projetos deste mesmo socket.
    """
    _sock(["kill-session", "-t", alvo_de_kill(name)])
    if not _existe(name):
        return True
    _log.warning("runner: kill-session nao derrubou %r — matando pelo pane_pid", name)
    # `=<nome>:` (sessao exata, janela ativa): alvo de pane/janela precisa do `=` E do `:` — sem
    # eles um nome numerico e lido como INDICE DE JANELA e a resposta vem de outra sessao.
    saida = _sock(["list-panes", "-t", f"={name}:", "-F", "#{pane_pid}"]).stdout
    for token in saida.split():
        if token.isdigit():
            try:
                # SIGKILL onde existe; no Windows o `os.kill` chama TerminateProcess com qualquer
                # sinal, e SIGKILL nao existe la. Um sinal so, sem ramo no corpo — este caminho so
                # e alcancado quando o multiplexador ja falhou.
                os.kill(int(token), getattr(signal, "SIGKILL", signal.SIGTERM))
            except (OSError, ValueError) as e:
                _log.debug("runner: nao matei o pane %s de %r: %r", token, name, e)
    # ESPERAR antes de reconferir, e o laco nao e enfeite: medido nesta VM, matar o `pane_pid`
    # derruba a sessao, mas nao no mesmo instante — perguntar na hora respondia "ainda viva" e o
    # play era recusado por um contorno que TINHA funcionado. Teto de ~2s, e so neste caminho
    # (o multiplexador ja falhou aqui; o play normal nao paga nada disto).
    for _ in range(20):
        if not _existe(name):
            return True
        time.sleep(0.1)
    return False


def _slug(cwd: str) -> str:
    base = re.sub(r"[^a-zA-Z0-9_-]", "-", Path(cwd).name) or "proj"
    return f"{base}-{hashlib.sha1(cwd.encode()).hexdigest()[:6]}"


def start_run(cwd: str, command: str) -> RunInfo:
    """Mata o run anterior do projeto (1 por projeto), inicia o novo numa sessao tmux
    no socket dedicado, grava como lembrado, devolve o status."""
    name = _slug(cwd)
    # ANTES de qualquer coisa que possa falhar: o comando lembrado e a INTENCAO da pessoa, e um
    # play que nao sobe nao pode custar a ela redigitar a linha. Na ordem antiga o `remember`
    # tambem rodava sempre — o que muda e que agora ha caminhos que levantam.
    remember(cwd, command)
    if not _matar_run(name):
        raise RunnerError(409, f"o run anterior nao morreu (sessao {name}); o novo NAO subiu — "
                               "o processo antigo segue rodando")
    # remain-on-exit ANTES do spawn (global no socket, que so tem runs): processo que morre
    # logo apos o play mantem pane+log e vira "failed" com exit code, em vez de sumir sem
    # rastro. Setar depois do new-session deixava exatamente essa janela aberta.
    _sock(["start-server"])
    _sock(["set-option", "-g", "remain-on-exit", "on"])
    spawn = _scope_prefix() + [
        "tmux", "-L", SOCK, "new-session", "-d", "-s", name, "-c", cwd, "-x", "200", "-y", "50",
        _linha_de_shell_no_pane(command),
    ]
    try:
        cp = RUN(spawn, capture_output=True, text=True, encoding="utf-8",
                 errors="replace", timeout=5)
    except (subprocess.TimeoutExpired, OSError) as e:
        raise RunnerError(502, f"o run nao subiu: {e}") from e
    if cp.returncode != 0:
        # O `rc` do `new-session` deixa de ser ignorado: era ele que carregava o "duplicate
        # session: <nome>" da medida acima — a unica pista de que o play nao aconteceu.
        motivo = (cp.stderr or "").strip() or f"rc={cp.returncode}"
        raise RunnerError(502, f"o run nao subiu: {motivo}")
    return run_status(cwd) or RunInfo(command=command)


def stop_run(cwd: str) -> None:
    """Para o run. Levanta `RunnerError` se a sessao SOBREVIVEU — dizer "parado" com o processo
    vivo e o mesmo defeito que o `projects.stop` ja combate no `stop_command`."""
    name = _slug(cwd)
    if not _matar_run(name):
        raise RunnerError(409, f"o run (sessao {name}) nao morreu — o processo segue rodando")


def all_runs() -> dict[str, RunInfo]:
    """Todos os runs do socket dedicado, por nome de sessao — INCLUSIVE os de pane morto
    (remain-on-exit), que carregam exited/exit_status. Uma chamada tmux so, pro /api/projects
    nao pagar um subprocesso por projeto."""
    cp = _sock(["list-panes", "-a", "-F",
                "#{session_name}\t#{session_created}\t#{pane_dead}\t#{pane_dead_status}"])
    out: dict[str, RunInfo] = {}
    if cp.returncode != 0:
        return out
    for line in cp.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 4:
            continue
        sn, created, dead, status = parts
        out[sn] = RunInfo(command="", since=int(created) if created.isdigit() else None,
                          exited=dead == "1",
                          # vazio quando vivo ou morto por sinal — ai nao ha exit code.
                          exit_status=int(status) if status.lstrip("-").isdigit() else None)
    return out


def run_status(cwd: str) -> Optional[RunInfo]:
    info = all_runs().get(_slug(cwd))
    if info:
        info.command = remembered(cwd) or ""
    return info


def run_pane(cwd: str) -> str:
    # Alvo de SESSAO literal: estas sessoes vivem no servidor `-L cppkt-run`, e o _pane_target resolve
    # pane contra o servidor default. Aqui nao ha janela extra pra desambiguar de qualquer forma.
    alvo = f"={_slug(cwd)}:"
    cp = _sock(["capture-pane", "-p", "-t", alvo, "-S", "-200"])
    return cp.stdout
