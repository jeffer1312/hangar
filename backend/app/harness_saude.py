"""Saúde dos harnesses instalados — uma linha por CLI, com o que o app espera dele e o conserto.

Cada peça que o app instala noutro CLI (hook, extensão, ponte de skills, login espalhado,
reconciliação de conta) já tem o instalador dela em algum módulo; o que faltava era UM lugar
que diga o que está fora do lugar e chame o instalador certo. As checagens são leitura barata
(existe? aponta pra onde? está na config?), e o conserto reusa o que a subida do backend e o
`install-claude-wrapper.sh` já rodam — nada aqui instala de um jeito novo.

`ok=None` é "não deu pra saber" (config ilegível), distinto de falha. O texto de cada item vai
como `codigo` + `params` e o front traduz (`harness_<codigo>`), regra do i18n do projeto.
"""
from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import time
import tomllib
from pathlib import Path

import sqlite3

from app import agentes_sync, contas, engine_probe, engines, hook_installer, kimi_hook_installer, oauth_codex, skill_bridge
from app.adapters.kimi.sessions import kimi_home
from app.agentes_sync import _codex_dir, provedor_embutido_do_pi
from app.config import list_config_dirs
from app.oauth_codex import PROVEDOR

_log = logging.getLogger("hangar.harness_saude")

_REPO = Path(__file__).resolve().parents[2]
_EXTENSOES_PI = ("hangar-state", "rich-status-line", "claude-bridge", "claude-todo",
                 "claude-hooks-adapter", "git-checkpoint", "fullscreen-tui")
_EXTENSOES_POR_CLI = {
    "pi": _EXTENSOES_PI,
    "omp": tuple(nome for nome in _EXTENSOES_PI if nome not in ("claude-todo", "fullscreen-tui")),
}
_HOOKS_CLAUDE = ("state_hook.py", "askq_capture.py", "preview_hook.py", "subagent_hook.py",
                 "pair_hook.py", "nav_hook.py")

_versoes: dict[str, tuple[float, str | None]] = {}


def _versao(cli: str) -> str | None:
    """`<cli> --version`, com cache: a versão não muda entre dois cliques e o comando custa ~0,3s."""
    hit = _versoes.get(cli)
    if hit and time.monotonic() - hit[0] < 600:
        return hit[1]
    v: str | None = None
    if shutil.which(cli):
        try:
            r = subprocess.run([cli, "-V" if cli == "tmux" else "--version"], capture_output=True, text=True, timeout=8,
                               encoding="utf-8", errors="replace")
            linha = (r.stdout or r.stderr or "").strip().splitlines()
            v = linha[0].strip() if linha else ""
        except (OSError, subprocess.SubprocessError) as e:
            _log.warning("harness: `%s --version` falhou: %r", cli, e)
            v = ""
    _versoes[cli] = (time.monotonic(), v)
    return v


def _item(id_: str, ok: bool | None, codigo: str, conserto: str | None = None, *, info: bool = False,
          **params) -> dict:
    """`info=True` é linha informativa (o que o CLI tem), não checagem: a tela desenha um ponto,
    não um ✓, e ela nunca pinta o card de vermelho."""
    return {"id": id_, "ok": ok, "codigo": codigo, "params": {k: str(v) for k, v in params.items()},
            "conserto": conserto, "info": info}


def _ler_json(p: Path) -> dict:
    try:
        d = json.loads(p.read_text(encoding="utf-8"))
        return d if isinstance(d, dict) else {}
    except (OSError, ValueError):
        return {}


def _ler_toml(p: Path) -> dict:
    try:
        return tomllib.loads(p.read_text(encoding="utf-8")) if p.is_file() else {}
    except (OSError, tomllib.TOMLDecodeError):
        return {}


# ---------------------------------------------------------------- informativos

def _mcp(cli: str) -> dict:
    home = Path.home()
    if cli == "claude":
        nomes = list((_ler_json(home / ".claude.json").get("mcpServers") or {}).keys())
    elif cli == "codex":
        nomes = list((_ler_toml(_codex_dir(None) / "config.toml").get("mcp_servers") or {}).keys())
    elif cli in ("pi", "omp"):
        nomes = list((_ler_json(_raiz_agente(cli) / "mcp.json").get("mcpServers") or {}).keys())
    else:
        cfg = _ler_toml(kimi_home() / "config.toml")
        nomes = list((cfg.get("mcp") or cfg.get("mcp_servers") or {}).keys())
    if not nomes:
        return _item("mcp", True, "mcp_nenhum", info=True)
    return _item("mcp", True, "mcp_ok", info=True, n=len(nomes), lista=", ".join(sorted(nomes)))


def _modelo_padrao(cli: str) -> dict:
    home = Path.home()
    if cli == "claude":
        m = _ler_json(home / ".claude" / "settings.json").get("model")
    elif cli == "codex":
        m = _ler_toml(_codex_dir(None) / "config.toml").get("model")
    elif cli in ("pi", "omp"):
        s = _ler_json(_raiz_agente(cli) / "settings.json")
        m = "/".join(x for x in (s.get("defaultProvider"), s.get("defaultModel")) if x) or None
    else:
        m = _ler_toml(kimi_home() / "config.toml").get("default_model")
    if not m:
        return _item("modelo", True, "modelo_padrao_nenhum", info=True)
    return _item("modelo", True, "modelo_padrao", info=True, modelo=str(m))


def _origem_das_skills(ponte: Path, home: Path) -> str:
    """Quantas skills da ponte vêm de cada fonte — o que responde 'e os plugins?' no Codex/Pi/Kimi:
    eles não têm plugin, recebem as skills dos plugins do Claude pela ponte."""
    contagem = {"plugins": 0, "pessoais": 0, "hangar": 0, "agents": 0}
    raizes = {
        "pessoais": os.path.normpath(str(home / ".claude" / "skills")),
        "plugins": os.path.normpath(str(home / ".claude" / "plugins")),
        "hangar": os.path.normpath(str(_REPO / "skills")),
        "agents": os.path.normpath(str(home / ".agents" / "skills")),
    }
    for p in ponte.iterdir():
        if not p.is_symlink():
            continue
        alvo = os.path.normpath(str(Path(os.readlink(p))))
        for nome, raiz in raizes.items():
            if alvo.startswith(raiz + os.sep):
                contagem[nome] += 1
                break
    return ", ".join(f"{v} {k}" for k, v in contagem.items() if v)


def _hooks_codex() -> dict:
    hooks = _ler_json(_codex_dir(None) / "hooks.json").get("hooks")
    if not isinstance(hooks, dict) or not hooks:
        return _item("hooks", True, "hooks_nenhum", info=True)
    n = sum(len(v) for v in hooks.values() if isinstance(v, list))
    return _item("hooks", True, "hooks_codex", info=True, n=n, eventos=", ".join(sorted(hooks)))


# ---------------------------------------------------------------- tmux

def _tmux(args: list[str]) -> str | None:
    try:
        r = subprocess.run(["tmux", *args], capture_output=True, text=True, timeout=5,
                           encoding="utf-8", errors="replace")
    except (OSError, subprocess.SubprocessError) as e:
        _log.warning("harness: tmux %s falhou: %r", " ".join(args), e)
        return None
    return r.stdout.strip() if r.returncode == 0 else None


def _card_tmux() -> dict:
    """As opções que o app EXIGE do tmux e por quê (o texto de cada uma mora no front). Lidas do
    servidor vivo, não do arquivo: é o que as sessões estão usando agora. O conserto único refaz o
    bloco gerenciado do ~/.tmux.conf e recarrega (install-claude-wrapper.sh --no-statusline)."""
    v = _versao("tmux")
    itens = []
    if v is None:
        return {"id": "tmux", "nome": "tmux", "instalado": False, "versao": None, "itens": []}
    conf = Path.home() / ".tmux.conf"
    try:
        bloco = "# >>> hangar >>>" in conf.read_text(encoding="utf-8")
    except OSError:
        bloco = False
    itens.append(_item("bloco", bloco, "tmux_bloco_ok" if bloco else "tmux_bloco_ausente", None if bloco else "tmux"))
    term = _tmux(["show", "-gv", "default-terminal"]) or ""
    ok_term = bool(term) and not term.startswith(("tmux", "screen"))
    itens.append(_item("default_terminal", ok_term, "tmux_term_ok" if ok_term else "tmux_term_ruim",
                       None if ok_term else "tmux", valor=term or "?"))
    env = _tmux(["show-environment", "-g"]) or ""
    ok_tc = "COLORTERM=truecolor" in env and "CLAUDE_CODE_TMUX_TRUECOLOR=1" in env
    itens.append(_item("truecolor", ok_tc, "tmux_truecolor_ok" if ok_tc else "tmux_truecolor_ruim",
                       None if ok_tc else "tmux"))
    titulos = _tmux(["show", "-gv", "set-titles-string"]) or ""
    ok_tit = titulos == "#S"
    itens.append(_item("titulo", ok_tit, "tmux_titulo_ok" if ok_tit else "tmux_titulo_ruim",
                       None if ok_tit else "tmux", valor=titulos or "?"))
    mouse = _tmux(["show", "-gv", "mouse"]) == "on"
    itens.append(_item("mouse", True, "tmux_mouse_on" if mouse else "tmux_mouse_off", info=True))
    tpm = (Path.home() / ".tmux" / "plugins" / "tmux-resurrect").is_dir()
    itens.append(_item("persistencia", True, "tmux_persist_on" if tpm else "tmux_persist_off", info=True))
    return {"id": "tmux", "nome": "tmux", "instalado": True, "versao": v, "itens": itens}


# ---------------------------------------------------------------- checagens

def _ponte_skills(nome: str, home: Path) -> dict:
    alvo = next((t for t in skill_bridge.TARGETS if t[0] == nome), None)
    if alvo is None:
        return _item("skills", None, "sem_ponte")
    _, rel_ponte, _, config_ok = alvo
    ponte = home / rel_ponte
    if not ponte.is_dir():
        return _item("skills", False, "ponte_ausente", "skills")
    links = [p for p in ponte.iterdir() if p.is_symlink()]
    mortos = [p for p in links if not p.exists()]
    if mortos:
        return _item("skills", False, "links_pendurados", "skills", n=len(mortos), total=len(links))
    if config_ok is not None:
        cfg = config_ok(home, ponte)
        if cfg is False:
            return _item("skills", False, "ponte_fora_da_config", n=len(links), cli=nome)
        if cfg is None:
            return _item("skills", None, "config_ilegivel")
    return _item("skills", True, "skills_ok", n=len(links), origem=_origem_das_skills(ponte, home))


def _raiz_agente(cli: str) -> Path:
    if cli == "omp":
        raiz = os.environ.get("PI_CODING_AGENT_DIR")
        return Path(raiz) if raiz else Path.home() / ".omp" / "agent"
    return Path.home() / ".pi" / "agent"


def _extensoes(cli: str) -> dict:
    raiz = _raiz_agente(cli)
    ext = raiz / "extensions"
    faltam = []
    # Link vivo pra outra fonte (o repo antigo das extensões, tipicamente): a extensão RODA, só não
    # é a daqui. Dizer "falta" pra isso contradiz a linha de fullscreen logo abaixo dizendo "ligado".
    outra_fonte = []
    for nome in _EXTENSOES_POR_CLI[cli]:
        p = ext / f"{nome}.ts"
        fonte = _REPO / "scripts" / "pi" / f"{nome}.ts"
        if p.is_symlink() and p.exists() and p.resolve() == fonte.resolve():
            continue
        if p.exists() and not p.is_symlink():
            continue  # arquivo do usuário com o mesmo nome: é dele, não conta como falta
        if p.is_symlink() and p.exists():
            outra_fonte.append(f"{nome} → {_abreviar_home(p.resolve())}")
            continue
        faltam.append(nome)
    if outra_fonte:
        params = {"lista": ", ".join(outra_fonte)}
        if faltam:
            params["faltam"] = ", ".join(faltam)
        return _item("extensoes", False, "extensoes_outra_fonte", f"extensoes:{cli}", **params)
    if faltam:
        return _item("extensoes", False, "faltam", f"extensoes:{cli}", lista=", ".join(faltam))
    return _item("extensoes", True, "extensoes_ok", n=len(_EXTENSOES_POR_CLI[cli]))


def _abreviar_home(p: Path) -> str:
    try:
        return "~/" + str(p.relative_to(Path.home()))
    except ValueError:
        return str(p)


def _fullscreen(cli: str) -> dict:
    """Alternate screen do Pi/omp dentro do tmux (extensão fullscreen-tui). Arquivo ausente =
    nunca ligado → conserto liga; `enabled: false` é escolha da pessoa (/fullscreen-off) e fica."""
    cfg = _raiz_agente(cli) / "fullscreen-tui.json"
    if not cfg.is_file():
        return _item("fullscreen", False, "fullscreen_desligado", f"fullscreen:{cli}")
    try:
        ligado = json.loads(cfg.read_text(encoding="utf-8")).get("enabled") is True
    except (OSError, ValueError, AttributeError):
        return _item("fullscreen", None, "config_ilegivel")
    return _item("fullscreen", True, "fullscreen_ok" if ligado else "fullscreen_por_escolha")


def _fullscreen_claude() -> dict:
    """`"tui": "fullscreen"` no settings.json do compartilhado (chave de topo: a reconciliação de
    conta a espelha pras cópias). Fora dele, dentro do tmux o Claude rola a tela toda a cada frame."""
    settings = contas.compartilhado() / "settings.json"
    if not settings.is_file():
        return _item("fullscreen", None, "config_ilegivel")
    d = _ler_json(settings)
    ligado = d.get("tui") == "fullscreen"
    return _item("fullscreen", ligado, "fullscreen_ok" if ligado else "fullscreen_claude_desligado",
                 None if ligado else "fullscreen:claude")


def _hooks_claude(dir_conta: Path) -> dict:
    settings = dir_conta / "settings.json"
    try:
        hooks = json.loads(settings.read_text(encoding="utf-8")).get("hooks") or {}
    except (OSError, ValueError, AttributeError):
        return _item("hooks", None, "config_ilegivel")
    texto = json.dumps(hooks)
    faltam = [h for h in _HOOKS_CLAUDE if h not in texto]
    if faltam:
        return _item("hooks", False, "faltam", "hooks-claude", lista=", ".join(faltam))
    return _item("hooks", True, "hooks_ok", n=len(_HOOKS_CLAUDE))


def _contas_claude() -> dict:
    nomes = contas.listar()
    if not nomes:
        return _item("contas", True, "so_conta_padrao")
    # Reconciliar é idempotente e barato: o botão fica sempre, é o "arruma agora" das contas.
    return _item("contas", True, "contas_ok", "contas", n=len(nomes), lista=", ".join(nomes))


def _plugins_claude() -> dict:
    """Plugins ligados no settings.json do compartilhado, conferidos contra o installed_plugins.json
    e a pasta de cada um — a mesma checagem que a reconciliação de conta faz."""
    try:
        ligados = json.loads((contas.compartilhado() / "settings.json").read_text(encoding="utf-8")).get("enabledPlugins") or {}
    except (OSError, ValueError, AttributeError):
        return _item("plugins", None, "config_ilegivel")
    nomes = sorted(n.split("@", 1)[0] for n, on in ligados.items() if on)
    try:
        avisos = contas._conferir_plugins(contas.compartilhado())
    except contas.ContaError:
        return _item("plugins", None, "config_ilegivel")
    if avisos:
        return _item("plugins", False, "plugins_com_problema", n=len(avisos), lista="; ".join(avisos))
    return _item("plugins", True, "plugins_ok", n=len(nomes), lista=", ".join(nomes))


def _hooks_kimi(home: Path) -> dict:
    cfg = home / "config.toml"
    try:
        data = tomllib.loads(cfg.read_text(encoding="utf-8")) if cfg.is_file() else {}
    except (OSError, tomllib.TOMLDecodeError):
        return _item("hooks", None, "config_ilegivel")
    faltam = kimi_hook_installer._missing(data)
    if faltam:
        return _item("hooks", False, "faltam_n", "hooks-kimi", n=len(faltam))
    return _item("hooks", True, "hooks_ok", n=len(kimi_hook_installer._ENTRIES))


# ---------------------------------------------------------------- credenciais por harness

def _no_harness(cli: str) -> set[str]:
    """Nomes de provedor que o harness já tem credencial gravada, no vocabulário DELE."""
    home = Path.home()
    tem: set[str] = set()
    try:
        if cli == "pi":
            d = home / ".pi" / "agent"
            tem |= set(json.loads((d / "auth.json").read_text(encoding="utf-8")).keys())
            if (d / "models.json").is_file():
                tem |= set((json.loads((d / "models.json").read_text(encoding="utf-8")).get("providers") or {}).keys())
        elif cli == "omp":
            db = oauth_codex._omp_db(None)
            if db.is_file():
                con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
                try:
                    tem |= {r[0] for r in con.execute("select provider from auth_credentials where disabled_cause is null")}
                finally:
                    con.close()
        elif cli == "kimi":
            cfg = kimi_home() / "config.toml"
            if cfg.is_file():
                provs = tomllib.loads(cfg.read_text(encoding="utf-8")).get("providers") or {}
                tem |= {n.removeprefix("managed:") for n in provs}
        elif cli == "codex":
            cfg = _codex_dir(None) / "config.toml"
            if cfg.is_file():
                tem |= set((tomllib.loads(cfg.read_text(encoding="utf-8")).get("model_providers") or {}).keys())
            if oauth_codex._codex_tem_login(None):
                tem.add(PROVEDOR)
    except (OSError, ValueError, sqlite3.Error, tomllib.TOMLDecodeError):
        pass
    return tem


def _do_app(cli: str) -> dict[str, dict | None]:
    """Credenciais que o app conhece, já com o nome que ESTE harness usaria: chave de API do
    engines.json (Pi/omp têm nome embutido por endereço; Kimi/Codex usam o nome do motor) e o
    login do ChatGPT, quando há um pra espalhar. Valor None = OAuth."""
    saida: dict[str, dict | None] = {}
    for nome, dados in engines.listar().items():
        base = dados.get("base_url") or ""
        if not base or not dados.get("api_key"):
            continue
        alvo = (provedor_embutido_do_pi(base) if cli in ("pi", "omp") else None) or nome
        saida[alvo] = {"nome": nome, "base_url": base, "api_key": dados["api_key"]}
    if cli != "kimi" and (oauth_codex.ler_cofre() or oauth_codex._codex_tem_login(None)):
        saida[PROVEDOR] = None
    return saida


def _credenciais(cli: str) -> dict:
    tem = _no_harness(cli)
    faltam = sorted(n for n in _do_app(cli) if n not in tem)
    lista = ", ".join(sorted(tem)) or "—"
    if faltam:
        return _item("credenciais", False, "credenciais_faltam", f"sync:{cli}", tem=lista, faltam=", ".join(faltam))
    return _item("credenciais", True, "credenciais_ok", tem=lista)


def _omp_gravar_chave(provedor: str, api_key: str) -> tuple[bool, str]:
    db = oauth_codex._omp_db(None)
    if not db.is_file():
        return False, "nao-instalado"
    try:
        con = sqlite3.connect(db, timeout=5)
        try:
            # Sem UNIQUE em provider: conferir antes, senão cada sync empilha outra linha.
            if con.execute("select 1 from auth_credentials where provider=? limit 1", (provedor,)).fetchone():
                return True, "ja-existe"
            con.execute("insert into auth_credentials (provider, credential_type, data) values (?, 'api_key', ?)",
                        (provedor, json.dumps({"key": api_key, "source": "manual"})))
            con.commit()
        finally:
            con.close()
    except sqlite3.Error as e:
        return False, f"sqlite: {e}"
    return True, str(db)


def _sincronizar(cli: str) -> str:
    """Grava no harness o que o app tem e ele não. Reusa o agentes_sync (pi/kimi/codex) e o
    propagar do OAuth; o omp guarda chave no mesmo SQLite do login."""
    tem = _no_harness(cli)
    feitos = []
    for alvo, cred in _do_app(cli).items():
        if alvo in tem:
            continue
        if cred is None:
            r = oauth_codex.propagar()
            feitos.append(f"{PROVEDOR}: {r.get(cli, {}).get('motivo', '?')}")
            continue
        if cli == "omp":
            ok, motivo = _omp_gravar_chave(alvo, cred["api_key"])
        else:
            try:
                modelos = engine_probe.listar_modelos(cred["base_url"], cred["api_key"])
            except Exception as e:  # noqa: BLE001 — lista de modelos é opcional
                _log.debug("sync %s: sem modelos para %s: %r", cli, cred["nome"], e)
                modelos = []
            r = agentes_sync.sincronizar(cred["nome"], cred["base_url"], cred["api_key"], modelos, (cli,))
            res = r.get(cli) or {"ok": False, "motivo": "alvo-desconhecido"}
            ok, motivo = res["ok"], res["motivo"]
        # O Codex guarda só o NOME da variável de ambiente: o motivo dele diz qual exportar, e
        # esconder isso atrás de "ok" seria prometer uma chave que ele ainda não vê.
        feitos.append(f"{alvo}: {motivo if (not ok or 'exporte' in motivo) else 'ok'}")
    return "; ".join(feitos) or "nada faltava"


def diagnosticar() -> list[dict]:
    home = Path.home()
    saida = []

    v = _versao("claude")
    itens = [_hooks_claude(Path(c.path)) for c in list_config_dirs() if Path(c.path, "settings.json").is_file()]
    # Uma linha só pros hooks: a pior conta manda (falta > ilegível > ok).
    hooks = (next((i for i in itens if i["ok"] is False), None) or next((i for i in itens if i["ok"] is None), None)
             or (itens[0] if itens else _item("hooks", None, "nenhuma_conta")))
    saida.append({"id": "claude", "nome": "Claude Code", "instalado": v is not None, "versao": v,
                  "itens": [hooks, _plugins_claude(), _fullscreen_claude(), _contas_claude(), _mcp("claude"),
                            _modelo_padrao("claude")]})

    v = _versao("codex")
    d = home / ".codex"
    saida.append({"id": "codex", "nome": "Codex", "instalado": v is not None or d.is_dir(), "versao": v,
                  "itens": [_credenciais("codex"), _ponte_skills("codex", home), _hooks_codex(), _mcp("codex"),
                            _modelo_padrao("codex")]
                  if d.is_dir() else []})

    v = _versao("pi")
    d = home / ".pi" / "agent"
    saida.append({"id": "pi", "nome": "Pi", "instalado": v is not None or d.is_dir(), "versao": v,
                  "itens": [_credenciais("pi"), _extensoes("pi"), _fullscreen("pi"), _ponte_skills("pi", home),
                            _mcp("pi"), _modelo_padrao("pi")]
                  if d.is_dir() else []})

    v = _versao("omp")
    d = _raiz_agente("omp")
    saida.append({"id": "omp", "nome": "oh-my-pi", "instalado": v is not None or d.is_dir(), "versao": v,
                  # Sem `_fullscreen("omp")`: a conversa do omp mora no scrollback do terminal por
                  # desenho — em alternate screen ela some e a roda vira seta (historico no composer).
                  "itens": [_credenciais("omp"), _extensoes("omp"), _mcp("omp"),
                            _modelo_padrao("omp")] if d.is_dir() else []})

    v = _versao("kimi")
    d = kimi_home()
    itens = []
    if d.is_dir():
        tem_status = (d / "statusline.js").is_file()
        itens = [_credenciais("kimi"), _hooks_kimi(d), _ponte_skills("kimi", home),
                 _item("statusline", tem_status, "statusline_ok" if tem_status else "sem_statusline"),
                 _mcp("kimi"), _modelo_padrao("kimi")]
    saida.append({"id": "kimi", "nome": "Kimi Code", "instalado": v is not None or d.is_dir(), "versao": v, "itens": itens})
    saida.append(_card_tmux())
    return saida


# ---------------------------------------------------------------- consertos

def _ligar_extensoes(cli: str) -> str:
    ext = _raiz_agente(cli) / "extensions"
    ext.mkdir(parents=True, exist_ok=True)
    feitos = []
    esperadas = _EXTENSOES_POR_CLI[cli]
    # Migra somente links nossos; configurações e extensões pessoais ficam intactas.
    for nome in _EXTENSOES_PI:
        if nome in esperadas:
            continue
        p = ext / f"{nome}.ts"
        fonte = _REPO / "scripts" / "pi" / f"{nome}.ts"
        if p.is_symlink() and p.resolve() == fonte.resolve():
            p.unlink()
    for nome in esperadas:
        p = ext / f"{nome}.ts"
        fonte = _REPO / "scripts" / "pi" / f"{nome}.ts"
        if not fonte.is_file() or (p.exists() and not p.is_symlink()):
            continue
        if p.is_symlink():
            p.unlink()
        p.symlink_to(fonte)
        feitos.append(nome)
    return f"{len(feitos)} extensões ligadas"


def consertar(id_: str) -> str:
    """Roda o conserto e devolve uma linha do que fez (saída de comando, não interface).
    Id desconhecido → ValueError."""
    if id_ == "skills":
        # O rebuild só diz POR QUE falhou pelo log: guardá-lo é o que separa "nada a fazer" de
        # "a ponte do kimi quebrou" — os dois davam a mesma linha de zeros.
        linhas: list[str] = []
        stats = skill_bridge.rebuild(log=lambda m: linhas.append(str(m)))
        if any("erro" in v for v in stats.values()):
            raise ValueError(" | ".join(l.strip() for l in linhas if "⚠" in l) or "rebuild falhou")
        return "; ".join(f"{k}: {v.get('criados', 0)} criados, {v.get('removidos', 0)} removidos"
                         for k, v in stats.items()) or "nada a fazer"
    if id_ == "hooks-claude":
        tocados: set[str] = set()
        for fn in (hook_installer.ensure_askq_hook_installed, hook_installer.ensure_state_hooks_installed,
                   hook_installer.ensure_preview_hook_installed, hook_installer.ensure_subagent_hook_installed,
                   hook_installer.ensure_pair_hook_installed, hook_installer.ensure_nav_hook_installed,
                   hook_installer.ensure_guard_hooks_installed):
            tocados.update(fn())
        return f"{len(tocados)} config dirs gravados"
    if id_ == "hooks-kimi":
        # `[]` de volta é "já estava" OU "falhou" (o instalador só loga); a releitura decide.
        gravou = kimi_hook_installer.ensure_kimi_hooks_installed()
        if _hooks_kimi(kimi_home())["ok"] is not True:
            raise ValueError("hooks do Kimi não ficaram instalados — ver o log do backend")
        return "config.toml gravado" if gravou else "já estava"
    if id_ == "contas":
        avisos = []
        for nome in contas.listar():
            avisos.extend(f"{nome}: {a}" for a in contas.reconciliar(nome))
        return "; ".join(avisos) or "contas reconciliadas, nada fora do lugar"
    if id_ == "oauth":
        if oauth_codex.ler_cofre() is None and oauth_codex.importar_do_codex() is None:
            raise ValueError("nenhum login do ChatGPT pra espalhar")
        r = oauth_codex.propagar()
        linha = "; ".join(f"{k}: {v['motivo']}" for k, v in r.items())
        # `nao-instalado` não é falha; qualquer outro alvo que não gravou é, mesmo com os
        # irmãos gravados — sucesso parcial contado como feito escondia o que ficou quebrado.
        if any(not v["ok"] and v["motivo"] != "nao-instalado" for v in r.values()):
            raise ValueError(linha)
        return linha
    if id_ == "tmux":
        # O bloco gerenciado é do instalador (bash); rodar o próprio instalador é o único jeito de
        # escrevê-lo igual ao de uma instalação nova. Idempotente, e o --no-statusline evita pergunta.
        bash = shutil.which("bash")
        if not bash:
            raise ValueError("sem bash nesta máquina — o bloco do tmux é do instalador POSIX")
        r = subprocess.run([bash, str(_REPO / "scripts" / "install-claude-wrapper.sh"), "--no-statusline"],
                           capture_output=True, text=True, timeout=120, encoding="utf-8", errors="replace",
                           cwd=str(_REPO))
        if r.returncode != 0:
            raise ValueError(f"instalador saiu com {r.returncode}: {(r.stderr or r.stdout)[-300:]}")
        return "bloco do ~/.tmux.conf refeito e recarregado"
    if id_ == "fullscreen:claude":
        settings = contas.compartilhado() / "settings.json"
        d = hook_installer._load_settings(settings)
        if d is None:
            raise ValueError("settings.json ilegível")
        d["tui"] = "fullscreen"
        hook_installer._write(settings, d)
        return "tui = fullscreen no settings.json; vale nas sessões novas"
    if id_ == "fullscreen:omp":
        # Tela cacheada de antes: o botao sumiu porque a rolagem do omp e a do scrollback.
        raise ValueError("o omp não tem tela cheia: a rolagem dele é a do terminal (ver CLAUDE.md)")
    if id_ == "fullscreen:pi":
        cfg = _raiz_agente("pi") / "fullscreen-tui.json"
        if cfg.exists():
            raise ValueError("já configurado — /fullscreen-on na TUI")
        cfg.parent.mkdir(parents=True, exist_ok=True)
        oauth_codex._gravar_json(cfg, {"enabled": True})
        return "fullscreen ligado; vale nas sessões novas"
    if id_ in ("sync:pi", "sync:omp", "sync:kimi", "sync:codex"):
        return _sincronizar(id_.split(":", 1)[1])
    # Só os dois agentes que o diagnóstico conhece: o id vem do cliente e vira caminho.
    if id_ in ("extensoes:pi", "extensoes:omp"):
        return _ligar_extensoes(id_.split(":", 1)[1])
    raise ValueError(f"conserto desconhecido: {id_}")
