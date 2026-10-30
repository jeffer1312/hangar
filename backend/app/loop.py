import json
import logging
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from app.config import settings
from app.pqueue import _sanitize

_log = logging.getLogger("hangar.loop")

ACTIVE = {"running", "paused_awaiting", "done_claimed"}

def _loop_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".hangar-loop"
    d.mkdir(parents=True, exist_ok=True)
    return d


class LoopLink:

    def __init__(self, name: str):
        self.path = _loop_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            return json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None

_REFINE_TIMEOUT = 60
class ClaudePError(Exception):
    """claude -p indisponivel/timeout/exit≠0/vazio. Compartilhado por refine_goal e ask-history;
    o endpoint mapeia pra 502."""


# O texto vem do body do usuario e vira prompt de um claude -p HEADLESS (sem TUI pra negar tool)
# rodando com o perfil do host -> prompt injection poderia executar tool real. Estes usos sao PURO
# TEXTO: nega toda ferramenta com efeito colateral (exec/edit/rede). --disallowedTools e variadico
# (nargs), entao NAO pode ter positional depois dele -> o prompt vai por STDIN, nunca no argv.
_REFINE_DISALLOWED = ("Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch")


def _exe_claude() -> str | None:
    """Caminho resolvido: no Windows o `claude` do npm e um `.CMD` (ver tests/test_cli_argv.py).
    Funcao propria pra teste trocar sem depender de haver `claude` na maquina."""
    return shutil.which("claude")


# O CLI escreve o aviso de limite no STDOUT ("You've hit your weekly limit · resets ...").
_LIMITE_RE = re.compile(r"hit your .*limit|usage limit|rate limit", re.IGNORECASE)
# Com exit 0 só conta como limite se a saída inteira FOR o aviso: resposta de verdade que cita
# "limit" não pode ser descartada.
_LIMITE_EXIT0_RE = re.compile(r"\s*(?:you'?ve hit your \w+ limit|claude ai usage limit reached|usage limit reached)",
                              re.IGNORECASE)
_LIMITE_EXIT0_MAX = 300


def _esgotada(c) -> bool:
    return any(j.pct >= 100 for j in c.janelas if not j.por_modelo)


def _ordem_contas(lidas: list) -> list[str | None]:
    """Config dirs na ordem de tentativa; None = conta padrão (herda o ambiente do backend).
    A padrão vai primeiro salvo se a leitura dela mostrar 5h/7d esgotada; as outras só entram
    com leitura boa (prova de login) e folga, pela maior sobra semanal."""
    padrao_esgotada = any(c.ativa and _esgotada(c) for c in lidas)
    outras = [c for c in lidas if c.provedor == "claude" and not c.ativa and c.estado == "lida"
              and c.janelas and not _esgotada(c)]
    outras.sort(key=lambda c: max((j.pct for j in c.janelas if j.rotulo == "7d"), default=0.0))
    ordem: list[str | None] = [] if padrao_esgotada else [None]
    ordem += [c.id.removeprefix("claude:") for c in outras]
    return ordem or [None]


def _rodar_claude_p(exe: str, prompt: str, conta: str | None) -> subprocess.CompletedProcess:
    env = None if conta is None else {**os.environ, "CLAUDE_CONFIG_DIR": conta}
    try:
        return subprocess.run(
            # Sem persistir: a conversa gravada em /tmp aparecia na busca como se fosse do usuário.
            [exe, "-p", "--model", "sonnet", "--no-session-persistence",
             "--disallowedTools", *_REFINE_DISALLOWED],
            input=prompt, cwd=tempfile.gettempdir(), capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=_REFINE_TIMEOUT, env=env,
        )
    except FileNotFoundError:
        raise ClaudePError("claude CLI não encontrado")
    except (subprocess.TimeoutExpired, OSError):
        raise ClaudePError("claude -p excedeu o tempo ou falhou ao iniciar")


def _bateu_limite(p: subprocess.CompletedProcess) -> bool:
    if p.returncode != 0:
        return bool(_LIMITE_RE.search(f"{p.stdout or ''}\n{p.stderr or ''}"))
    out = (p.stdout or "").strip()
    return len(out) <= _LIMITE_EXIT0_MAX and bool(_LIMITE_EXIT0_RE.match(out))


def _motivo(p: subprocess.CompletedProcess) -> str:
    # Sem stderr (caso do limite), a causa está no stdout.
    return ((p.stderr or "").strip() or (p.stdout or "").strip())[-500:]


def _nome(conta: str | None) -> str:
    return conta or "padrão"


def _avisar_leituras(lidas: list) -> None:
    """Deixa no log quando a escolha da conta foi feita sem dado confiável."""
    from app import cotas

    if not any(c.ativa and c.estado == "lida" and c.janelas for c in lidas):
        _log.warning("claude -p: sem leitura de cota da conta padrão (%d leituras no cache); "
                     "escolha caiu na padrão sem dados", len(lidas))
    agora = time.time()
    velhas = [c for c in lidas if c.ts is not None and agora - c.ts > cotas._TTL_S]
    if velhas:
        _log.info("claude -p: escolhendo com leitura de cota velha: %s",
                  ", ".join(f"{c.label} ({int(agora - c.ts)}s)" for c in velhas))


def _claude_p(prompt: str) -> str:
    """Roda um claude -p efemero (sonnet) com o prompt por STDIN, tools de efeito colateral negadas,
    cwd neutro (tempdir), argv sem shell, timeout 60s. Devolve o stdout (strip). Levanta ClaudePError
    em qualquer falha (CLI ausente/timeout/exit≠0/vazio/limite) — o endpoint mapeia pra 502.
    Roda na conta com cota (`_ordem_contas`); bateu limite, tenta UMA vez a próxima."""
    from app import cotas  # tardio: cotas puxa meio app, e loop é importado cedo

    exe = _exe_claude()
    if exe is None:
        raise ClaudePError("claude CLI não encontrado")
    lidas = cotas.cotas_claude()
    _avisar_leituras(lidas)
    conta = _ordem_contas(lidas)[0]
    p = _rodar_claude_p(exe, prompt, conta)
    if _bateu_limite(p):
        erro_limite = f"claude -p: conta {_nome(conta)} bateu o limite: {_motivo(p)}"
        _log.warning("%s", erro_limite)
        try:
            # A leitura em cache pode ser velha: relê antes de escolher a próxima.
            lidas = cotas.cotas_claude(atualizar=True)
        except Exception as e:  # noqa: BLE001 - falha da releitura não pode esconder o limite
            _log.warning("claude -p: releitura das cotas falhou: %s", e)
            raise ClaudePError(erro_limite) from e
        proximas = [c for c in _ordem_contas(lidas) if c != conta]
        if not proximas:
            raise ClaudePError(erro_limite)
        _log.info("claude -p: tentando a conta %s", _nome(proximas[0]))
        conta = proximas[0]
        p = _rodar_claude_p(exe, prompt, conta)
        if p.returncode != 0 or _bateu_limite(p):
            raise ClaudePError(f"{erro_limite}; segunda tentativa na conta {_nome(conta)} falhou "
                               f"(exit {p.returncode}): {_motivo(p)}")
    if p.returncode != 0:
        raise ClaudePError(f"claude -p falhou (exit {p.returncode}): {_motivo(p)}")
    _log.info("claude -p atendido pela conta %s", _nome(conta))
    out = (p.stdout or "").strip()
    if not out:
        raise ClaudePError("claude -p devolveu saída vazia")
    return out
