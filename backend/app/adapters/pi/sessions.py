"""Onde uma sessao do Pi vive no disco.

O Pi persiste toda sessao em JSONL, igual ao Claude, e aceita um id escolhido pelo caller
(`pi --session-id <id>`) — e por isso que o adapter dele e irmao do ClaudeAdapter e nao do
CodexAdapter: retomar e reabrir um arquivo, nao ressuscitar um app-server.

UMA diferenca de layout importa: o nome do arquivo tem timestamp na frente
(`<ts>_<uuid>.jsonl`), entao o path NAO e derivavel so do session-id como no Claude. Resolver
exige glob pelo sufixo.
"""
import os
import re
from pathlib import Path

# Copia de getDefaultSessionDirPath (pi-coding-agent 0.82.1, dist/core/session-manager.js:245):
#     `--${resolvedCwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--`
# So separador de caminho vira '-'. Espaco, acento, '_' e ponto passam INTACTOS — a versao antiga
# daqui trocava todo nao-alfanumerico e so acertava caminho ASCII simples: pra
# `/home/jefferson/Área de trabalho/...` ela devolvia `--home-jefferson--rea-de-trabalho-...--`,
# diretorio que nao existe, e o fallback por CP_PI_SESSION morria calado (a sessao so tinha
# transcript enquanto o bilhete do pane estivesse bom).
_SEP_RE = re.compile(r"[/\\:]")
_LEADING_SEP_RE = re.compile(r"^[/\\]")


def sessions_root() -> Path:
    # `--session-dir` do CLI sobrescreve o env; aqui so o env importa, porque as sessoes que o app
    # cria sao spawnadas por nos, sem esse flag.
    env = os.environ.get("PI_CODING_AGENT_SESSION_DIR")
    if env:
        return Path(env)
    return Path.home() / ".pi" / "agent" / "sessions"


def cwd_slug(cwd: str) -> str:
    # `resolvePath` do Pi = absoluto com '~' expandido; um cwd relativo daria outro diretorio.
    resolved = os.path.abspath(os.path.expanduser(cwd))
    return "--" + _SEP_RE.sub("-", _LEADING_SEP_RE.sub("", resolved, count=1)) + "--"


# Subagente do Pi: o transcript dele NAO fica ao lado do da sessao — vai pra
# `<stem-da-sessao>/<taskId>/run-<n>/session.jsonl` (medido no Pi 0.82.1, numa sessão real:
# `2026-07-30T20-29-24-651Z_18e48e08-…/44bad0fb/run-2/session.jsonl` ao lado do
# `2026-07-30T20-29-24-651Z_18e48e08-….jsonl` da conversa de verdade).
# Os dois sinais valem OU: se o Pi renomear o arquivo, o diretorio `run-<n>` ainda denuncia; se
# largar o `run-<n>`, o nome fixo `session.jsonl` ainda denuncia. Nenhum transcript de sessao cai
# em qualquer um dos dois — o dele leva timestamp+uuid no nome e mora direto no slug do cwd.
# ponytail: calibration knob — se o layout do Pi mudar, e AQUI que ajusta.
_RUN_DIR_RE = re.compile(r"^run-\d+$")


def is_subagent_transcript(path: str) -> bool:
    """O JSONL e de um subagente (Task tool do Pi), nao da conversa da sessao?"""
    p = Path(path)
    return p.name == "session.jsonl" or any(_RUN_DIR_RE.match(d) for d in p.parts[:-1])


def root_transcript(path: str) -> str:
    """Do transcript de um subagente pro da conversa, ou "" se nao der pra provar quem e a raiz.

    O proprio caminho ja diz: o diretorio que segura os runs tem o nome do arquivo da sessao sem o
    `.jsonl` (`…_18e48e08-….jsonl` e `…_18e48e08-…/44bad0fb/run-2/session.jsonl` sao vizinhos).
    Subir por aqui e melhor que resolver de novo pelo cwd — nao depende do slug nem do
    CP_PI_SESSION, entao funciona tambem depois de um /fork.
    """
    for anc in list(Path(path).parents)[:4]:      # <stem>/<taskId>/run-N/ = 3 niveis; 4 e a folga
        cand = Path(f"{anc}.jsonl")
        if cand.is_file():
            return str(cand)
    return ""


def transcript_path(cwd: str, session_id: str) -> str:
    """Caminho do JSONL da sessao, ou "" se ela ainda nao escreveu nada.

    "" e nao excecao: o registry chama isto logo depois do spawn, e a TUI so cria o arquivo no
    primeiro turno. Levantar aqui transformaria "sessao novinha" em erro.
    """
    d = sessions_root() / cwd_slug(cwd)
    if not d.is_dir():
        return ""
    cands = sorted(d.glob(f"*_{session_id}.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)
    return str(cands[0]) if cands else ""
