"""Modo de permissão do Claude Code que o Python ainda lê: o padrão da conta e o modo fora do plano
da sessão (transcript e registro de abertura). Ler e trocar o modo no pane é do Rust."""

import json
import logging
import threading
from collections import OrderedDict
from pathlib import Path

from app import atomico

_log = logging.getLogger("hangar.permissao")

# Modo de quem criou a sessão sem escolher um na tela. Sem isto o valor fica nulo, a flag não vai
# no comando e a sessão nasce no que a máquina tiver — numa sem `defaultMode` isso é pedir
# permissão a cada ferramenta. Acesso total é o que o app já dá aos outros agentes.
PADRAO_DO_APP = "bypassPermissions"


def modo_da_conta(config_dir: str | None) -> str:
    """`permissions.defaultMode` da conta, ou o padrão do app quando ela não define nenhum."""
    from app import model_args

    arquivo = (Path(config_dir).expanduser() if config_dir else Path.home() / ".claude") / "settings.json"
    try:
        dados = json.loads(arquivo.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return PADRAO_DO_APP   # conta sem settings.json é o caso NORMAL: nada a avisar
    except (OSError, ValueError) as exc:
        # Ilegível não é o mesmo que ausente: o padrão é o modo mais permissivo que existe, e cair
        # nele por arquivo quebrado parece escolha de quem abriu a sessão.
        _log.warning("permissão: %s ilegível (%s) — sessão nasce em %s", arquivo, exc, PADRAO_DO_APP)
        return PADRAO_DO_APP
    modo = ((dados.get("permissions") or {}) if isinstance(dados, dict) else {}).get("defaultMode")
    if modo == "default":
        return "manual"   # o nome que a CLI aceita na flag; "default" só existe no settings.json
    return modo if modo in model_args.MODOS_PERMISSAO_CLAUDE else PADRAO_DO_APP

# O último modo fora do plano visto por sessão.
_ultimos_nao_plan: OrderedDict[str, str] = OrderedDict()
_operacoes_controladas: dict[str, int] = {}
_mem_lock = threading.Lock()


def _launch_mode(session_id: str) -> str | None:
    """Modo da abertura no registro mais recente da conversa (o transcript só o grava na 1ª fala); sem `plan`."""
    achados = []
    for path in (Path.home() / ".hangar" / "session-specs").glob("*.json"):
        try:
            spec = json.loads(atomico.read(path))
        except (OSError, ValueError):
            continue
        if spec.get("conversation") == session_id and spec.get("permission_mode") not in (None, "plan"):
            achados.append((spec.get("created_at") or 0, spec["permission_mode"]))
    return max(achados, default=(0, None))[1]


def known_non_plan(name: str) -> str | None:
    """Só devolve a base observada nesta sessão, sem assumir a configuração da conta."""
    with _mem_lock:
        return _ultimos_nao_plan.get(name)


# Os fatos do terminal perguntam a cada tique enquanto há recado na fila; a varredura chega a 8 MB.
_transcript_modes: dict[str, tuple[tuple[int, int], str | None]] = {}


def transcript_non_plan_mode(jsonl: str) -> str | None:
    """`permissionMode` mais recente fora de `plan` que o próprio CLI gravou no transcript. `plan`
    é pulado porque o Claude Code conta plan vindo de bypass como bypass."""
    from app.worktrees import reversed_lines

    try:
        st = Path(jsonl).stat()
    except OSError:
        return None
    version = (st.st_size, st.st_mtime_ns)
    cached = _transcript_modes.get(jsonl)
    if cached is not None and cached[0] == version:
        return cached[1]
    found = None
    try:
        for raw in reversed_lines(jsonl):
            if b'"permissionMode"' not in raw:
                continue
            try:
                modo = json.loads(raw).get("permissionMode")
            except ValueError:
                continue
            if isinstance(modo, str) and modo not in ("", "plan"):
                found = modo
                break
    except OSError:
        return None
    if len(_transcript_modes) >= 256:
        _transcript_modes.clear()
    _transcript_modes[jsonl] = (version, found)
    return found


def session_non_plan_mode(jsonl: str | None) -> str | None:
    """Modo fora de `plan` da sessão Claude do transcript `jsonl`: o que o pane mostrou (a memória é
    gravada pelo session-id, o stem do jsonl, como o monitor faz) ou, sem ele, o do transcript.
    A memória vem antes porque um Shift+Tab só chega ao transcript na fala seguinte; e só ela não
    basta, porque existe apenas para sessão com monitor aberto desde o último restart."""
    if not jsonl:
        return None
    sid = Path(jsonl).stem
    # A abertura é o último recurso: o que foi visto ou gravado depois dela vale mais.
    return known_non_plan(sid) or transcript_non_plan_mode(jsonl) or _launch_mode(sid)


def operacao_em_curso(sessao: str) -> bool:
    with _mem_lock:
        return sessao in _operacoes_controladas
