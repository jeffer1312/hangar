import json
import os
import shlex
import sys
from pathlib import Path

from app.config import list_config_dirs, _backend_config_base

# Caminho absoluto do script de captura (resolvido uma vez). O command vai literal pro
# settings.json do Claude, entao precisa ser absoluto — o hook roda com cwd arbitrario.
HOOK = str((Path(__file__).parent.parent / "hooks" / "askq_capture.py").resolve())
# `python3` nao existe no Windows (la o binario e python.exe; `python3` e um alias da Microsoft
# Store que ABRE A LOJA em vez de rodar o hook). sys.executable ainda garante o Python do venv do
# backend em vez de torcer pelo que estiver no PATH quando o Claude dispara o hook. Aspas nos dois
# porque qualquer um dos caminhos pode ter espaco (C:\Program Files\..., "Application Support").
# `|| exit 0`: hook nosso e TELEMETRIA (estado, previa, captura) — falha dele NUNCA pode virar
# prompt bloqueado. Sem isso, um venv apagado na maquina (o backend subiu de uma worktree que
# depois perdeu o .venv — incidente de 17/08/2026 na maquina de casa) fazia o Claude Code recusar
# TODO prompt da maquina com "operation blocked by hook". Vale em sh e no cmd do Windows.
_FALHA_NAO_BLOQUEIA = " || exit 0"
_COMMAND = f'"{sys.executable}" "{HOOK}"{_FALHA_NAO_BLOQUEIA}'
_MATCHER = "AskUserQuestion"


def _script_of(command: str) -> str:
    """Caminho do script dentro de um command nosso ('"py" "X"' -> 'X'). E o ultimo token
    terminado em .py — o command carrega um `|| exit 0` atras, entao "ultimo token" cru
    devolveria `0` e o _sync_hook passaria a casar hooks alheios."""
    toks = _tokens(command)
    return next((t.strip("\"'") for t in reversed(toks) if t.strip("\"'").endswith(".py")),
                toks[-1])


def _tokens(command: str) -> list[str]:
    try:
        return shlex.split(command) or [command]
    except ValueError:  # aspas desbalanceadas a mao
        return command.split() or [command]


def _refers_to(command: object, script: str, por_nome: bool = False) -> bool:
    """True se este hook e NOSSO: o command referencia o arquivo `script`, nao importa o
    formato ('python3 X', '"/venv/bin/python3" "X"', com ou sem aspas).

    Comparar a string INTEIRA foi o que duplicou os hooks quando o formato do command
    mudou — a entrada antiga deixou de ser reconhecida e uma nova foi acrescentada.

    `por_nome=True` afrouxa pro NOME do arquivo, e e o que TODO hook nosso usa hoje. O casamento
    por CAMINHO tratava outro checkout do repo como outro hook, e o settings.json e um so pra
    maquina: subir o backend de uma worktree (`.worktrees/<x>/backend/hooks/state_hook.py`)
    acrescentava uma segunda copia de cada hook ao lado da do checkout principal, e nenhum dos
    dois installers removia a do outro — todo evento passava a rodar o mesmo script 2x, e apagar
    a mao voltava na proxima subida. A trava do tmux (guard_tmux.py) ja tinha caido nisso por
    outro caminho (repo -> symlink em <config>/hooks/, pra passar na allowlist do pi, mais dois
    config dirs compartilhando o mesmo settings.json) e por isso foi a primeira a vir pra ca.

    O preco e explicito: com por_nome so existe UMA entrada de cada hook por settings.json, entao
    dois backends de checkouts diferentes nao recebem estado ao mesmo tempo — quem subiu por
    ultimo fica com o hook. Pra hook de maquina isso e o certo."""
    if not isinstance(command, str):
        return False
    alvo = os.path.basename(script) if por_nome else script
    return any(
        (os.path.basename(t.strip("\"'")) if por_nome else t.strip("\"'")) == alvo
        for t in _tokens(command)
    )


def _load_settings(settings_path: Path) -> dict | None:
    """Le o settings.json. None = arquivo quebrado/estranho a mao -> nao mexer nele.
    Perder a config do usuario seria pior que nao instalar o hook."""
    data: dict = {}
    if settings_path.exists():
        raw = settings_path.read_text(encoding="utf-8").strip()
        if raw:
            try:
                data = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                return None  # JSON invalido editado a mao -> NAO clobbra
        if not isinstance(data, dict):
            return None  # raiz nao-objeto
    hooks = data.get("hooks")
    if hooks is not None and not isinstance(hooks, dict):
        return None  # 'hooks' nao e objeto
    return data


def _sync_hook(
    data: dict, event: str, command: str, matcher: str | None = None, por_nome: bool = False
) -> bool:
    """Deixa EXATAMENTE UMA entrada nossa sob hooks[event], com o command atual.
    Formato antigo e substituido no lugar; duplicatas colapsam na primeira ocorrencia
    (o installer roda a cada subida do backend, entao ele cura a bagunca que criou).
    Hooks de terceiros ficam intactos, na mesma ordem. Retorna True se mudou algo."""
    script = _script_of(command)
    hooks = data.setdefault("hooks", {})
    ev_list = hooks.setdefault(event, [])
    if not isinstance(ev_list, list):
        return False  # lista do evento estranha -> nao mexe

    changed = False
    kept = False
    empties: list[int] = []
    for block in ev_list:
        if not isinstance(block, dict):
            continue
        entries = block.get("hooks")
        if not isinstance(entries, list):
            continue
        surviving = []
        for h in entries:
            if isinstance(h, dict) and _refers_to(h.get("command"), script, por_nome):
                if kept:
                    changed = True  # duplicata nossa -> descarta
                    continue
                kept = True
                if h.get("command") != command:
                    h["command"] = command  # formato antigo -> atual, no lugar
                    changed = True
                h.setdefault("type", "command")
            surviving.append(h)
        if len(surviving) != len(entries):
            if surviving:
                block["hooks"] = surviving
            else:
                empties.append(id(block))  # bloco que era so nosso duplicado
    if empties:
        ev_list = [b for b in ev_list if id(b) not in empties]
        hooks[event] = ev_list
    if not kept:
        block: dict = {"hooks": [{"type": "command", "command": command}]}
        if matcher is not None:
            block["matcher"] = matcher
        ev_list.append(block)
        changed = True
    return changed


def _write(settings_path: Path, data: dict) -> None:
    settings_path.write_text(
        json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )


def _ensure_settings_file(settings_path: Path) -> bool:
    """Garante o bloco PreToolUse/AskUserQuestion num unico settings.json, PRESERVANDO
    todo o resto: outros hooks do usuario (GateGuard, caveman, ponytail, matcher 'Bash'…)
    e qualquer outra chave (model, env, permissions…). Retorna True se gravou (mudou)."""
    data = _load_settings(settings_path)
    if data is None:
        return False
    if not _sync_hook(data, "PreToolUse", _COMMAND, matcher=_MATCHER, por_nome=True):
        return False
    _write(settings_path, data)
    return True


STATE_HOOK = str((Path(__file__).parent.parent / "hooks" / "state_hook.py").resolve())
_STATE_COMMAND = f'"{sys.executable}" "{STATE_HOOK}"{_FALHA_NAO_BLOQUEIA}'  # mesma razao do _COMMAND acima
# SessionStart inclui o caso de abrir ja com --resume/clear (fixa o transcript ativo na hora); os
# demais eventos cobrem o /resume feito DENTRO da sessao (marca no 1o prompt/tool depois dele).
_STATE_EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop", "SessionStart"]


def _ensure_event_hook(
    settings_path: Path,
    event: str,
    command: str,
    matcher: str | None = None,
    por_nome: bool = False,
) -> bool:
    """Garante UMA entrada {command} sob settings['hooks'][event], preservando todo o resto.
    Mesma blindagem do _ensure_settings_file: settings.json quebrado/estranho e PULADO (False)."""
    data = _load_settings(settings_path)
    if data is None:
        return False
    if not _sync_hook(data, event, command, matcher=matcher, por_nome=por_nome):
        return False
    _write(settings_path, data)
    return True


def ensure_state_hooks_installed() -> list[str]:
    """Instala (idempotente) o state_hook nos 5 eventos, em cada config dir. Fail-soft por arquivo;
    nunca derruba o startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            changed = False
            for ev in _STATE_EVENTS:
                if _ensure_event_hook(d / "settings.json", ev, _STATE_COMMAND, por_nome=True):
                    changed = True
            if changed:
                touched.append(str(d))
        except Exception:
            continue
    return touched


PREVIEW_HOOK = str((Path(__file__).parent.parent / "hooks" / "preview_hook.py").resolve())
_PREVIEW_COMMAND = f'"{sys.executable}" "{PREVIEW_HOOK}"{_FALHA_NAO_BLOQUEIA}'  # mesma razao do _COMMAND acima
# MessageDisplay = os deltas do texto em voo (Claude Code >= 2.1.152); Stop = zera a previa no fim
# do turno ("" e resposta, nao ausencia — ver o contrato em app/preview.py).
_PREVIEW_EVENTS = ["MessageDisplay", "Stop"]


def ensure_preview_hook_installed() -> list[str]:
    """Instala (idempotente) o publicador de previa do Claude nos 2 eventos, em cada config dir.
    Fail-soft por arquivo, como os demais. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            changed = False
            for ev in _PREVIEW_EVENTS:
                if _ensure_event_hook(d / "settings.json", ev, _PREVIEW_COMMAND, por_nome=True):
                    changed = True
            if changed:
                touched.append(str(d))
        except Exception:
            continue
    return touched


SUBAGENT_HOOK = str((Path(__file__).parent.parent / "hooks" / "subagent_hook.py").resolve())
_SUBAGENT_COMMAND = f'"{sys.executable}" "{SUBAGENT_HOOK}"{_FALHA_NAO_BLOQUEIA}'
# Os DOIS eventos: `Start` é o que faz o subagente aparecer enquanto roda (que é o ponto do painel),
# e `Stop` é o que traz `agent_transcript_path` e a última mensagem. Só com o Stop, um subagente de
# 5 minutos ficaria invisível justo enquanto está trabalhando.
_SUBAGENT_EVENTS = ["SubagentStart", "SubagentStop"]


def ensure_subagent_hook_installed() -> list[str]:
    """Instala (idempotente) o publicador de subagentes nos 2 eventos, em cada config dir.

    Mesma forma do preview: fail-soft por arquivo, casado POR NOME do script (por_nome=True) pra
    não duplicar a entrada quando o caminho do repo mudar de lugar.
    """
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            changed = False
            for ev in _SUBAGENT_EVENTS:
                if _ensure_event_hook(d / "settings.json", ev, _SUBAGENT_COMMAND, por_nome=True):
                    changed = True
            if changed:
                touched.append(str(d))
        except Exception:
            continue
    return touched


GUARD_HOOK = str((Path(__file__).parent.parent / "hooks" / "guard_tmux.py").resolve())


def _guard_path(config_dir: Path) -> str:
    """Caminho do guard a registrar no settings.json daquele config dir.

    Preferimos um symlink em `<config>/hooks/guard_tmux.py` em vez do caminho do repo por causa
    do **pi**: o adaptador dele (`~/.pi/agent/claude-hooks-adapter.json`) so roda hooks cujo
    command casa a allowlist, e o DEFAULT dela e `/.claude/hooks/`, `/Projetos/skills/`,
    `rtk hook` — um command apontando pro checkout do hangar e simplesmente ignorado la, sem
    erro nenhum (a pior falha possivel: parece protegido e nao esta). Pelo symlink a trava vale
    nos tres motores sem editar config de ninguem.

    Symlink falhou (Windows sem privilegio, FS sem suporte)? Cai no caminho do repo: Claude e
    Kimi continuam protegidos e o pi fica de fora — pior que o ideal, melhor que sem trava."""
    link = config_dir / "hooks" / "guard_tmux.py"
    try:
        link.parent.mkdir(parents=True, exist_ok=True)
        if link.is_symlink() and os.readlink(link) == GUARD_HOOK:
            return str(link)
        if link.exists() or link.is_symlink():
            link.unlink()
        link.symlink_to(GUARD_HOOK)
        return str(link)
    except OSError:
        return GUARD_HOOK


def ensure_guard_hooks_installed() -> list[str]:
    """Instala (idempotente) a trava de `tmux kill-server` sem -L/-S em cada config dir.

    Vive AQUI, no instalador do app, e nao no ~/.claude de quem escreveu: a maquina que roda o
    hangar e justamente a que tem varias sessoes tmux vivas pra perder, e subagente nasce sem
    memoria e sem regra de CLAUDE.md de quem o criou. Fail-soft igual aos outros."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    vistos: set[Path] = set()
    # Ordem DETERMINISTICA, com `.claude` na frente: quando varios config dirs dividem o mesmo
    # settings.json, quem escreve primeiro define o caminho que fica gravado — e o pi so roda
    # hook cujo command casa `/.claude/hooks/`. Iterando o set cru, o sorteio as vezes gravava
    # `~/.claude-jefferson/hooks/...` e a trava sumia silenciosamente no pi.
    for d in sorted(dirs, key=lambda p: (p.name != ".claude", str(p))):
        try:
            if not d.is_dir():
                continue
            settings = d / "settings.json"
            # Dois config dirs desta maquina apontam pro MESMO settings.json (um e symlink do
            # outro). Sem esta chave por arquivo resolvido, cada dir escrevia a sua entrada la
            # dentro e a trava aparecia duplicada.
            chave = settings.resolve()
            if chave in vistos:
                continue
            vistos.add(chave)
            comando = f'"{sys.executable}" "{_guard_path(d)}"'
            if _ensure_event_hook(settings, "PreToolUse", comando, matcher="Bash", por_nome=True):
                touched.append(str(d))
        except Exception:
            continue
    return touched


def ensure_askq_hook_installed() -> list[str]:
    """Instala (idempotente) o hook PreToolUse de captura do AskUserQuestion no settings.json
    de cada config dir do Claude. Fail-soft por arquivo: um settings.json problematico nunca
    derruba o backend no startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()}
    except Exception:
        return []  # descoberta de dirs falhou (ex: HOME ausente) -> startup NUNCA quebra
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            if _ensure_settings_file(d / "settings.json"):
                touched.append(str(d))
        except Exception:
            continue  # installer de startup NUNCA propaga excecao
    return touched
