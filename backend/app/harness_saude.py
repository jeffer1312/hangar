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
                 "claude-hooks-adapter", "git-checkpoint")
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
            r = subprocess.run([cli, "--version"], capture_output=True, text=True, timeout=8,
                               encoding="utf-8", errors="replace")
            linha = (r.stdout or r.stderr or "").strip().splitlines()
            v = linha[0].strip() if linha else ""
        except (OSError, subprocess.SubprocessError):
            v = ""
    _versoes[cli] = (time.monotonic(), v)
    return v


def _item(id_: str, ok: bool | None, codigo: str, conserto: str | None = None, **params) -> dict:
    return {"id": id_, "ok": ok, "codigo": codigo, "params": {k: str(v) for k, v in params.items()},
            "conserto": conserto}


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
    return _item("skills", True, "skills_ok", n=len(links))


def _raiz_agente(cli: str) -> Path:
    if cli == "omp":
        raiz = os.environ.get("PI_CODING_AGENT_DIR")
        return Path(raiz) if raiz else Path.home() / ".omp" / "agent"
    return Path.home() / ".pi" / "agent"


def _extensoes(cli: str) -> dict:
    raiz = _raiz_agente(cli)
    ext = raiz / "extensions"
    faltam = []
    for nome in _EXTENSOES_PI:
        p = ext / f"{nome}.ts"
        fonte = _REPO / "scripts" / "pi" / f"{nome}.ts"
        if p.is_symlink() and p.exists() and p.resolve() == fonte.resolve():
            continue
        if p.exists() and not p.is_symlink():
            continue  # arquivo do usuário com o mesmo nome: é dele, não conta como falta
        faltam.append(nome)
    if faltam:
        return _item("extensoes", False, "faltam", f"extensoes:{cli}", lista=", ".join(faltam))
    return _item("extensoes", True, "extensoes_ok", n=len(_EXTENSOES_PI))


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
            except Exception:  # noqa: BLE001 — lista de modelos é opcional
                modelos = []
            r = agentes_sync.sincronizar(cred["nome"], cred["base_url"], cred["api_key"], modelos, (cli,))
            ok, motivo = r[cli]["ok"], r[cli]["motivo"]
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
                  "itens": [hooks, _plugins_claude(), _contas_claude()]})

    v = _versao("codex")
    d = home / ".codex"
    saida.append({"id": "codex", "nome": "Codex", "instalado": v is not None or d.is_dir(), "versao": v,
                  "itens": [_credenciais("codex"), _ponte_skills("codex", home)]
                  if d.is_dir() else []})

    v = _versao("pi")
    d = home / ".pi" / "agent"
    saida.append({"id": "pi", "nome": "Pi", "instalado": v is not None or d.is_dir(), "versao": v,
                  "itens": [_credenciais("pi"), _extensoes("pi"), _ponte_skills("pi", home)]
                  if d.is_dir() else []})

    v = _versao("omp")
    d = _raiz_agente("omp")
    saida.append({"id": "omp", "nome": "oh-my-pi", "instalado": v is not None or d.is_dir(), "versao": v,
                  "itens": [_credenciais("omp"), _extensoes("omp")] if d.is_dir() else []})

    v = _versao("kimi")
    d = kimi_home()
    itens = []
    if d.is_dir():
        tem_status = (d / "statusline.js").is_file()
        itens = [_credenciais("kimi"), _hooks_kimi(d), _ponte_skills("kimi", home),
                 _item("statusline", tem_status, "statusline_ok" if tem_status else "sem_statusline")]
    saida.append({"id": "kimi", "nome": "Kimi Code", "instalado": v is not None or d.is_dir(), "versao": v, "itens": itens})
    return saida


# ---------------------------------------------------------------- consertos

def _ligar_extensoes(cli: str) -> str:
    ext = _raiz_agente(cli) / "extensions"
    ext.mkdir(parents=True, exist_ok=True)
    feitos = []
    for nome in _EXTENSOES_PI:
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
        stats = skill_bridge.rebuild(log=lambda *_: None)
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
        return "config.toml gravado" if kimi_hook_installer.ensure_kimi_hooks_installed() else "já estava"
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
        if not any(v["ok"] for v in r.values()):
            raise ValueError(linha)
        return linha
    if id_ in ("sync:pi", "sync:omp", "sync:kimi", "sync:codex"):
        return _sincronizar(id_.split(":", 1)[1])
    # Só os dois agentes que o diagnóstico conhece: o id vem do cliente e vira caminho.
    if id_ in ("extensoes:pi", "extensoes:omp"):
        return _ligar_extensoes(id_.split(":", 1)[1])
    raise ValueError(f"conserto desconhecido: {id_}")
