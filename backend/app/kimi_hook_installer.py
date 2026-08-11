"""Instalador do hook de estado do Kimi (kimi_state_hook.py) no config.toml do usuario.

Irmao do hook_installer.py (Claude), mas o Kimi le hooks de `~/.kimi-code/config.toml` em TOML
(array `[[hooks]]` com event/matcher/command/timeout), nao de settings.json. TOML nao se edita em
memoria como JSON sem uma lib de escrita (tomllib so LE), entao a estrategia e APPEND textual:
preserva o arquivo do usuario byte a byte (ele tem providers e chaves ali dentro) e `[[hooks]]`
no fim do arquivo e sempre uma posicao de statement valida.

Idempotencia: antes de apendar, parseia com tomllib e procura o NOSSO script nos commands ja
configurados (casa por caminho, nao pela string inteira — mesma licao do _refers_to do Claude).
Roda a cada subida do backend (main.py), entao um append faltante se cura sozinho.

Nunca quebra o startup: config ausente -> cria; config invalido -> pula (NAO clobba arquivo do
usuario); hooks em forma inline (`hooks = [...]`, fora da doc) -> pula com warning (apendar
`[[hooks]]` depois de um array inline seria redefinicao e o TOML nao abriria mais).
"""
import logging
import shlex
import shutil
import sys
import tomllib
from pathlib import Path

from app.adapters.kimi.sessions import kimi_home

_log = logging.getLogger("claude_pocket.kimi_hook_installer")

HOOK = str((Path(__file__).parent.parent / "hooks" / "kimi_state_hook.py").resolve())
# sys.executable = o python do venv do backend (mesma razao do hook_installer do Claude).
_COMMAND = f'"{sys.executable}" "{HOOK}"'
# Entradas (evento, matcher): SessionStart/UserPromptSubmit/TurnStarted/Stop/Interrupt/
# PermissionRequest cobrem working/idle/awaiting_input + o bilhete pane->sessao; PreToolUse e
# PostToolUse SOMENTE com matcher AskUserQuestion: a pergunta nativa nao dispara PermissionRequest
# (medido — ver backend/hooks/kimi_state_hook.py) e sem estes o marcador ficava "working" com o
# agente parado no picker.
_ENTRIES = [("SessionStart", None), ("UserPromptSubmit", None), ("TurnStarted", None),
            ("Stop", None), ("Interrupt", None), ("PermissionRequest", None),
            ("PreToolUse", "AskUserQuestion"), ("PostToolUse", "AskUserQuestion")]


def _blocks(entries: list[tuple[str, str | None]]) -> str:
    lines = ["", "# hangar: marcadores de estado + bilhete pane->sessao (app/kimi_hook_installer).",
             "# Remova este bloco para desligar o rastreio de sessoes Kimi pelo app."]
    for ev, matcher in entries:
        # TOML literal string (aspas simples): nao escapa nada e nenhum path nosso tem apostrofo —
        # o command tem aspas duplas, que numa basic string precisariam de escaping.
        lines.append("[[hooks]]")
        lines.append(f'event = "{ev}"')
        if matcher is not None:
            lines.append(f'matcher = "{matcher}"')
        lines.append(f"command = '{_COMMAND}'")
        lines.append("timeout = 5")
        lines.append("")
    return "\n".join(lines) + "\n"


def _refers_to(command: object) -> bool:
    """True se este command aponta pro NOSSO script (qualquer formato de aspas), como o _refers_to
    do hook_installer do Claude: comparar a string inteira foi o que duplicou hooks la."""
    if not isinstance(command, str):
        return False
    try:
        tokens = shlex.split(command) or [command]
    except ValueError:
        tokens = command.split() or [command]
    return any(t.strip("\"'") == HOOK for t in tokens)


def _missing(data: dict) -> list[tuple[str, str | None]]:
    """Entradas nossas que AINDA nao estao no config. Idempotencia POR ENTRADA (evento+matcher):
    o conjunto cresceu uma vez ja (PreToolUse/PostToolUse de AskUserQuestion entraram depois) e um
    check de "tem algum hook nosso" pularia a instalacao das novas pra sempre."""
    hooks = data.get("hooks")
    if not isinstance(hooks, list):
        return list(_ENTRIES)
    out = []
    for ev, matcher in _ENTRIES:
        found = any(
            isinstance(h, dict) and h.get("event") == ev and h.get("matcher") == matcher
            and _refers_to(h.get("command"))
            for h in hooks
        )
        if not found:
            out.append((ev, matcher))
    return out


def ensure_kimi_hooks_installed() -> list[str]:
    """Garante nossos [[hooks]] no config.toml do Kimi (idempotente, fail-soft).
    Retorna o path se gravou (so pra log), [] senao."""
    try:
        home = kimi_home()
        if not home.is_dir():
            return []  # kimi nunca rodou nesta maquina -> nada a fazer
        cfg = home / "config.toml"
        raw = cfg.read_text(encoding="utf-8") if cfg.exists() else ""
        if raw.strip():
            try:
                data = tomllib.loads(raw)
            except tomllib.TOMLDecodeError:
                _log.warning("kimi: config.toml invalido; hook NAO instalado (arquivo intacto)")
                return []
            missing = _missing(data)
            if not missing:
                return []
            if missing and "hooks" in data and "[[hooks]]" not in raw:
                # hooks definido como array inline (fora da doc oficial): apendar [[hooks]] seria
                # redefinicao e quebraria o TOML. Melhor sem hook que sem config.
                _log.warning("kimi: hooks inline no config.toml; hook NAO instalado")
                return []
            # Backup UMA vez antes da primeira mexida (mesmo espirito do .bak que o proprio kimi faz).
            bak = home / "config.toml.bak-hangar"
            if not bak.exists():
                shutil.copyfile(cfg, bak)
        else:
            missing = list(_ENTRIES)
        out = raw
        if out and not out.endswith("\n"):
            out += "\n"
        out += _blocks(missing)
        tmp = cfg.with_suffix(".toml.tmp-hangar")
        tmp.write_text(out, encoding="utf-8")
        tmp.replace(cfg)  # atomico
        return [str(cfg)]
    except Exception:
        _log.exception("kimi: falha ao instalar hook (fail-soft)")
        return []
