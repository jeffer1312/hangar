import json
import os
import shlex
from pathlib import Path

from app import install_root
from app.config import list_config_dirs, _backend_config_base

# Pela raiz da instalação: com o pacote, o `current` sem resolver, para o hook gravado sobreviver à
# troca e à poda da versão; sem ele, o checkout e o Python deste backend, como antes.
_HOOKS_DIR = install_root.root() / "backend" / "hooks"
_PYTHON = install_root.interpreter()
# Caminho absoluto do script de captura (resolvido uma vez). O command vai literal pro
# settings.json do Claude, entao precisa ser absoluto — o hook roda com cwd arbitrario.
HOOK = str(_HOOKS_DIR / "askq_capture.py")
# `python3` nao existe no Windows (la o binario e python.exe; `python3` e um alias da Microsoft
# Store que ABRE A LOJA em vez de rodar o hook). sys.executable ainda garante o Python do venv do
# backend em vez de torcer pelo que estiver no PATH quando o Claude dispara o hook. Aspas nos dois
# porque qualquer um dos caminhos pode ter espaco (C:\Program Files\..., "Application Support").
# `|| exit 0`: hook nosso e TELEMETRIA (estado, previa, captura) — falha dele NUNCA pode virar
# prompt bloqueado. Sem isso, um venv apagado na maquina (o backend subiu de uma worktree que
# depois perdeu o .venv — incidente de 17/08/2026 na maquina de casa) fazia o Claude Code recusar
# TODO prompt da maquina com "operation blocked by hook". Vale em sh e no cmd do Windows.
_FALHA_NAO_BLOQUEIA = " || exit 0"
# Nestes dois eventos o stdout de um hook que sai com 0 entra no contexto do modelo: a falha deixa
# de sumir calada sem voltar a bloquear o prompt. Quem imprime e o shell, entao vale ate com o
# script ou o Python sumido. Texto puro e ASCII: JSON em aspas simples nao roda no cmd do Windows.
_EVENTOS_COM_CONTEXTO = {"SessionStart", "UserPromptSubmit"}


def _falha_avisa(command: str, event: str) -> str:
    """O command com o sufixo de falha do evento: aviso onde o modelo le, `exit 0` no resto."""
    if event not in _EVENTOS_COM_CONTEXTO or not command.endswith(_FALHA_NAO_BLOQUEIA):
        return command
    # Nome sem `.py`: o _script_of acha o script pelo ultimo token que termina assim.
    nome = os.path.basename(_script_of(command)).removesuffix(".py")
    return (f'{command.removesuffix(_FALHA_NAO_BLOQUEIA)} || echo "[hangar] O hook {nome} do Hangar '
            f'falhou em {event} e foi ignorado. AVISE O USUARIO agora: parte do app pode estar sem '
            f'estado, pareamento ou navegador. Para ver o erro, rode o comando do hook a mao."')


_COMMAND =f'"{_PYTHON}" "{HOOK}"{_FALHA_NAO_BLOQUEIA}'
_MATCHER = "AskUserQuestion"


def _script_of(command: str) -> str:
    """Caminho do script dentro de um command nosso ('"py" "X"' -> 'X'). E o ultimo token
    terminado em .py — o command carrega um `|| exit 0` atras, entao "ultimo token" cru
    devolveria `0` e o _sync_hook passaria a casar hooks alheios."""
    toks = _tokens(command)
    return next((t.strip("\"'") for t in reversed(toks) if t.strip("\"'").endswith(".py")),
                toks[-1])


def _tokens(command: str) -> list[str]:
    r"""Tokens do command, respeitando aspas — e no Windows respeitando a CONTRABARRA.

    `shlex.split` em modo POSIX trata `\` como escape FORA de aspas: um command no formato antigo
    (`python3 C:\cockpit\backend\hooks\askq_capture.py`, sem aspas) sai como
    `C:cockpitackendhooksskq_capture.py` — o caminho perde os separadores e `_refers_to` deixa de
    reconhecer o hook como NOSSO. O efeito e o que este modulo inteiro existe pra impedir: hook
    duplicado, agora um por subida do backend, porque o instalador nao acha o que ele mesmo
    escreveu. (Dentro de aspas o POSIX preserva a contrabarra, entao o formato de hoje escapava
    disso por sorte, nao por desenho.)

    `posix=False` mantem as aspas nos tokens; quem compara ja faz `.strip("\"'")`.
    """
    try:
        return shlex.split(command, posix=(os.name != "nt")) or [command]
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
    data: dict, event: str, command: str | None, matcher: str | None = None, por_nome: bool = False, script: str | None = None
) -> bool:
    """Deixa EXATAMENTE UMA entrada nossa sob hooks[event], com o command atual (`None`: nenhuma).
    Formato antigo e substituido no lugar; duplicatas colapsam na primeira ocorrencia
    (o installer roda a cada subida do backend, entao ele cura a bagunca que criou).
    Hooks de terceiros ficam intactos, na mesma ordem. Retorna True se mudou algo."""
    script = script or _script_of(command)
    hooks = data.setdefault("hooks", {})
    ev_list = hooks.setdefault(event, [])
    if not isinstance(ev_list, list):
        return False  # lista do evento estranha -> nao mexe

    changed = False
    kept = command is None  # sem command, toda entrada nossa sai como duplicata
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
    """Garante os blocos AskUserQuestion (PreToolUse grava a pergunta; PostToolUse e
    PostToolUseFailure a apagam quando ela termina no terminal) num unico settings.json, PRESERVANDO
    todo o resto: outros hooks do usuario (GateGuard, caveman, ponytail, matcher 'Bash'…)
    e qualquer outra chave (model, env, permissions…). Retorna True se gravou (mudou)."""
    data = _load_settings(settings_path)
    if data is None:
        return False
    changed = False
    for event in ("PreToolUse", "PostToolUse", "PostToolUseFailure"):
        changed |= _sync_hook(data, event, _COMMAND, matcher=_MATCHER, por_nome=True)
    if not changed:
        return False
    _write(settings_path, data)
    return True


STATE_HOOK = str(_HOOKS_DIR / "state_hook.py")
_STATE_COMMAND = f'"{_PYTHON}" "{STATE_HOOK}"{_FALHA_NAO_BLOQUEIA}'  # mesma razao do _COMMAND acima
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
    if not _sync_hook(data, event, _falha_avisa(command, event), matcher=matcher, por_nome=por_nome):
        return False
    _write(settings_path, data)
    return True


def ensure_state_hooks_installed() -> list[str]:
    """Instala (idempotente) o state_hook nos 5 eventos, em cada config dir. Fail-soft por arquivo;
    nunca derruba o startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
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


PREVIEW_HOOK = str(_HOOKS_DIR / "preview_hook.py")
_PREVIEW_COMMAND = f'"{_PYTHON}" "{PREVIEW_HOOK}"{_FALHA_NAO_BLOQUEIA}'  # mesma razao do _COMMAND acima
# MessageDisplay = os deltas do texto em voo (Claude Code >= 2.1.152); Stop = zera a previa no fim
# do turno ("" e resposta, nao ausencia — ver o contrato em app/preview.py).
_PREVIEW_EVENTS = ["MessageDisplay", "Stop"]


def ensure_preview_hook_installed() -> list[str]:
    """Instala (idempotente) o publicador de previa do Claude nos 2 eventos, em cada config dir.
    Fail-soft por arquivo, como os demais. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
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


SUBAGENT_HOOK = str(_HOOKS_DIR / "subagent_hook.py")
_SUBAGENT_COMMAND = f'"{_PYTHON}" "{SUBAGENT_HOOK}"{_FALHA_NAO_BLOQUEIA}'
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
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
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


GUARD_HOOK = str(_HOOKS_DIR / "guard_tmux.py")


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
        # realpath nos dois lados, e nao `os.readlink(link) == GUARD_HOOK`: no Windows o readlink
        # devolve o alvo com o prefixo de caminho estendido (\\?\C:\...), entao a igualdade por
        # string nunca dava certo e o symlink era refeito a cada subida do backend. Mesmo motivo
        # (e mesma correcao) do contas._aponta_para, onde o estrago era maior.
        if not (link.is_symlink() and os.path.realpath(link) == os.path.realpath(GUARD_HOOK)):
            if link.exists() or link.is_symlink():
                link.unlink()
            link.symlink_to(GUARD_HOOK)
        # Pasta `hooks/` da conta é link pra do principal: grava o caminho do principal. Com o da
        # conta, o espelho do settings (contas._espelhar_do_principal) e este instalador trocavam o
        # comando um do outro a cada subida/abertura, e toda sessão sem terminal da conta acusava
        # "config mudou".
        return str(Path(os.path.realpath(link.parent)) / link.name)
    except OSError:
        return GUARD_HOOK


def ensure_guard_hooks_installed() -> list[str]:
    """Instala (idempotente) a trava de `tmux kill-server` sem -L/-S em cada config dir.

    Vive AQUI, no instalador do app, e nao no ~/.claude de quem escreveu: a maquina que roda o
    hangar e justamente a que tem varias sessoes tmux vivas pra perder, e subagente nasce sem
    memoria e sem regra de CLAUDE.md de quem o criou. Fail-soft igual aos outros."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
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
            comando = f'"{_PYTHON}" "{_guard_path(d)}"'
            if _ensure_event_hook(settings, "PreToolUse", comando, matcher="Bash", por_nome=True):
                touched.append(str(d))
        except Exception:
            continue
    return touched


# startup|resume|clear|compact: todo caminho em que o contexto nasce/renasce sem o prompt do --pair.
_PAIR_MATCHER = "startup|resume|clear|compact"


def ensure_pair_hook_installed() -> list[str]:
    """Instala (idempotente) o `hangar-server pair-context` no SessionStart de cada config dir, no lugar
    do antigo pair_hook.py; sem o binario, nenhum. Fail-soft por arquivo. Retorna os dirs onde gravou."""
    from app import diag, rust_bins
    try:
        from app.pair import _pair_dir
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
        command = (exe := rust_bins.find_bin("hangar-server", "CP_RUST_SERVER_BIN")) and f'"{exe}" pair-context "{_pair_dir()}"{_FALHA_NAO_BLOQUEIA}'
    except Exception as e:
        diag.registrar("grupos.hook_falhou", "erro", codigo="pair_hook_setup_failed", **diag.erro_campos(e))
        return []
    if command is None:
        diag.registrar("grupos.hook_sem_binario", "aviso")
    touched: list[str] = []
    for d in dirs:
        try:
            if not d.is_dir():
                continue
            data = _load_settings(d / "settings.json")
            if data is None:
                diag.registrar("grupos.hook_falhou", "erro", codigo="pair_hook_settings_unreadable", pasta=d.name)
                continue
            if (_sync_hook(data, "SessionStart", None, por_nome=True, script="pair_hook.py")
                    | _sync_hook(data, "SessionStart", command, _PAIR_MATCHER, True, "pair-context")):
                _write(d / "settings.json", data)
                touched.append(str(d))
        except Exception as e:
            diag.registrar("grupos.hook_falhou", "erro", codigo="pair_hook_write_failed", pasta=d.name, **diag.erro_campos(e))
    return touched


NAV_HOOK = str(_HOOKS_DIR / "nav_hook.py")
_NAV_COMMAND = f'"{_PYTHON}" "{NAV_HOOK}"{_FALHA_NAO_BLOQUEIA}'


def ensure_nav_hook_installed() -> list[str]:
    """Instala (idempotente) o aviso de navegador embutido no UserPromptSubmit, em cada config dir.
    A cada prompt, e nao so na abertura: o navegador abre e fecha no meio da sessao."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
    except Exception:
        return []
    touched: list[str] = []
    for d in dirs:
        try:
            if d.is_dir() and _ensure_event_hook(d / "settings.json", "UserPromptSubmit", _NAV_COMMAND,
                                                 por_nome=True):
                touched.append(str(d))
        except Exception:
            continue
    return touched


def ensure_askq_hook_installed() -> list[str]:
    """Instala (idempotente) o hook PreToolUse de captura do AskUserQuestion no settings.json
    de cada config dir do Claude. Fail-soft por arquivo: um settings.json problematico nunca
    derruba o backend no startup. Retorna os dirs onde gravou (so pra log)."""
    try:
        dirs = {Path(c.path) for c in list_config_dirs(ordered=False)} | {_backend_config_base().resolve()}
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
