import asyncio
import collections
import json
import logging
import os
import re
import secrets
import shutil
import sys
import threading
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Callable, Optional
from app import atomico, diag, share_store, shortcut_terminals, tmux
from app import agentpane
from app import permission_mode as modo_permissao
from app.config import settings
from app.mensagens import erro
from app import claude_customizations as session_customizations
from app import plugin_bridge
from app import runtime_config
from app.names import sanitize_session_name
from app.git_ops import git_summary, git_diffstat
from app import worktrees
from app.models import SessionInfo, session_key
from app.pqueue import PromptQueue, _sanitize, merged_history
from app.archive import _texto_simples
from app.chain import ThenLink
from app import guest_users, pair
from app.pair import PairLink, rename_pair, leave as pair_leave
from app.adapters.claude_headless import sessions as headless_sessions
from app.adapters.codex import sessions as codex_sessions
from app.adapters.orq import runs as orq_runs
from app import codex_contas
from app.askquestion import clear_pending_askq, pergunta_aberta
from app.state import (classify, _live_spinner, rate_limit_reset, corrige_ocioso_kimi,
                       aprovacao_kimi, codex_turno_aberto, menu_codex,
                       status_line as _pane_status)
from app import claude_context
from app.statusline import read as _sidecar_status, escolhas as _escolhas_status
from app.adapters.codex.adapter import status_line_do_rollout as _codex_status_line
from app.hook_state import hook_state
from app.planprog import plan_progress, plano_escondido
# As funcoes de /proc vivem no procinfo.py — e o unico ponto do backend preso ao Linux.
# Importadas por NOME (nao `procinfo._cmdline(...)`) de proposito: os testes fazem
# monkeypatch delas neste modulo, e o binding local preserva isso.
from app import model_args
from app import procinfo
from app.procinfo import (_proc_children_map, _descendant_pids, _open_jsonl, _cmdline, _argv,
                         _config_dir_of, _proc_start_time, _engine_of)
# _proc_environ_path/_proc_stat_path saem de proposito da lista acima: existem SO como ponto de
# injecao de teste e sao chamadas de dois lados (aqui e de dentro do procinfo). Importadas por
# nome, cada lado ganharia um binding proprio e um monkeypatch alcancaria so um deles — o outro
# leria o /proc real e o teste passaria por acidente. Qualificadas pelo modulo, ha UM alvo.

# Sentinela: distingue "pid nao informado" (resolve sozinho via tmux) de "pid=None" (sem pane).
_UNSET = object()

_log = logging.getLogger("hangar.registry")

# Vezes que a descoberta Python rodou, por caminho. Com o Rust dono fica zerada: é a prova de que
# nenhum caminho a chama (`tests/test_list_consumers.py`).
PYTHON_DISCOVERY: collections.Counter[str] = collections.Counter()


def rust_owns_list() -> bool:
    """Com o Rust dono, a descoberta e o cache de resolução do transcript são dele (`list_bridge`).
    `pending` espera o desfecho até `PENDING_WAIT_S`; no próprio laço não há como esperar. Sem
    desfecho, levanta com código: a lista nunca passa ao Python por isso."""
    from app import list_bridge, runtime_coordinator
    owner = runtime_coordinator.current()
    if owner is None:
        return False
    if owner.mode == "pending" and not runtime_coordinator._mode_bypass.get():
        from app.runtime_adapter import run_sync
        try:
            run_sync(owner.await_mode, owner.loop)
        except (runtime_coordinator.RuntimeStarting, TimeoutError) as e:
            raise list_bridge.ListBridgeError("list_runtime_starting") from e
        except RuntimeError as e:
            # Chamada de dentro do laço: esperar ali travaria o próprio desfecho.
            raise list_bridge.ListBridgeError("list_wait_on_loop") from e
    # Ainda `pending` depois da espera = adoção das sessões no Rust que acabou de subir (o
    # bypass da espera): ele já está de pé e a descoberta é dele.
    return owner.mode != "python"


async def rust_owns_list_async() -> bool:
    from app import list_bridge, runtime_coordinator
    owner = runtime_coordinator.current()
    if owner is None:
        return False
    if owner.mode == "pending" and not runtime_coordinator._mode_bypass.get():
        try:
            await owner.await_mode()
        except runtime_coordinator.RuntimeStarting as e:
            raise list_bridge.ListBridgeError("list_runtime_starting") from e
    return owner.mode != "python"


def _rust_caches() -> bool:
    """Semear, esquecer e invalidar só valem com o Rust de pé: em `pending` o próximo Rust nasce
    com os caches vazios, e esperar por ele aqui só atrasaria a criação."""
    from app import runtime_coordinator
    owner = runtime_coordinator.current()
    return owner is not None and owner.mode == "rust"


# Idade minima de um marcador awaiting_input pra que um pane raspado SEM menu o rebaixe pra idle
# (hook_state.demote_awaiting). O grace cobre a janela Notification->menu renderizado: raspar nesse
# vao nao pode matar um awaiting real que ainda nem apareceu na tela.
_AWAITING_DEMOTE_GRACE_S = 10.0
# Folga para as entradas de sistema que o Claude Code grava logo depois do Stop.
_IDLE_STALE_S = 1.0

# Teto de pares (done, total) por sessao em plan_tasks. O front so segmenta a barra com <= 8 Tasks
# (PlanBar.svelte), acima disso desenha barra unica e ignora a lista. 9 e nao 8 DE PROPOSITO: cortar
# em 8 exatos faria o front achar que o plano TEM 8 Tasks e segmentar um plano de 30.
_MAX_PLAN_TASK_SEGMENTS = 9


# Ultimo git bom por cwd: (summary, diffstat). Resultado None (repo sumiu, timeout) NAO apaga o
# anterior — erro nunca vira "repositorio limpo" no card.
_git_ultimo: dict[str, tuple[dict | None, dict | None]] = {}
_git_em_voo: set[str] = set()
# Pool próprio: git lento não pode deixar na fila a captura do tmux e o resto do pool padrão.
_git_pool = ThreadPoolExecutor(max_workers=4, thread_name_prefix="hangar-git")


def _git_dir(info) -> str | None:
    """Onde o git da sessão roda (`SessionInfo.git_dir`), aceitando os objetos mínimos da lista."""
    return getattr(info, "git_cwd", None) or info.cwd


async def _atualizar_git(cwd: str) -> None:
    try:
        summary, diffstat = await asyncio.get_running_loop().run_in_executor(
            _git_pool, lambda: (git_summary(cwd), git_diffstat(cwd)))
        antes = _git_ultimo.get(cwd, (None, None))
        _git_ultimo[cwd] = (summary if summary is not None else antes[0],
                            diffstat if diffstat is not None else antes[1])
    except Exception:
        _log.exception("git em segundo plano falhou cwd=%s (mantido o ultimo numero)", cwd)
    finally:
        _git_em_voo.discard(cwd)


def _pair_external(name: str, peers_: list[str] | None) -> dict | None:
    from app import external_pairs
    for r in external_pairs.by_local(name):
        if r.address in (peers_ or []):
            return {"alias": r.alias, "owner": r.peer_owner, "session": r.peer_session}
    return None


def _encerrar_pares_externos(morta: str) -> None:
    """Sessão morta fora do app: o par externo dela não tem quem o desfaça. Registro e convite
    saem na hora; o aviso ao outro lado vai em thread solta, porque isto roda dentro do list()."""
    from app import external_pairs
    alvo = [r for r in external_pairs.all() if r.local_session == morta or _sanitize(r.local_session) == morta]
    for r in alvo:
        for fn, arg in ((external_pairs.remove, r.share_id), (share_store.revoke, r.share_id)):
            try:
                fn(arg)
            except OSError as e:
                _log.warning("varredura de pares: par externo '%s' não limpo: %r", r.address, e)
        threading.Thread(target=_avisar_par_externo, args=(r,), daemon=True).start()


def _log_leave_warnings(name: str, warnings) -> None:
    """Saída do grupo antigo na criação: os avisos não têm a quem voltar, mas não somem calados."""
    if isinstance(warnings, list) and warnings:
        # Só a contagem: o aviso pode trazer texto da outra máquina.
        _log.warning("criação de %s: a saída do grupo antigo deixou %d aviso(s)", name, len(warnings))


def _avisar_par_externo(rec) -> None:
    from app import external_pairs, peers
    try:
        external_pairs.call(rec.peer_address, rec.peer_token, "DELETE", "/api/pair")
    except (peers.PeerError, ValueError) as e:
        _log.warning("varredura de pares: outro lado de '%s' não avisado: %s", rec.address, e)


def _decorate_loop(info) -> None:
    """Decora loop_status/iter/max de UMA sessao a partir do sidecar (app.loop). Sem loop -> tudo None
    (sem badge). Module-level (nao closure) pra ser testavel isolado."""
    from app.loop import LoopLink
    d = LoopLink(info.name).get()
    if d is not None:
        info.loop_status = d.get("status")
        info.loop_iter = d.get("iter")
        info.loop_max = d.get("max_iters")


def _decorate_plan(info) -> None:
    """Decora plan_* de UMA sessao a partir do .md do plano (app.planprog). Sem plano -> tudo None.
    Engole a excecao de proposito: roda no tick da lista, e uma falha aqui nao pode derrubar o SSE
    (incidente 2026-07-23). Module-level (nao closure) pra ser testavel isolado, igual _decorate_loop."""
    try:
        p = plan_progress(info.cwd)
    except Exception:
        _log.warning("decorate_plan falhou pra %r", getattr(info, "name", "?"), exc_info=True)
        return
    if p is None:
        # Sem plano tem DOIS motivos: nao existe, ou o usuario escondeu. So o 2o mantem o painel
        # (e o seletor que desfaz) na tela. Custo: um read do pin, so pra sessao sem plano.
        info.plan_hidden = plano_escondido(info.cwd) or None
        return
    info.plan_name = p.name
    info.plan_task = p.task_idx
    info.plan_task_total = p.task_total
    info.plan_done = p.done
    info.plan_total = p.total
    info.plan_complete = p.complete
    # Sem o corte, um plano de 30 Tasks manda 30 pares por sessao em TODO /api/sessions e em toda
    # re-emissao do SSE, de graca. Ver _MAX_PLAN_TASK_SEGMENTS acima pro porque de 9 e nao 8.
    info.plan_tasks = [(t.done, t.total) for t in p.tasks[:_MAX_PLAN_TASK_SEGMENTS]]


def _orq_infos() -> list[SessionInfo]:
    """Orquestrações `auto` vivas: sem pane nem processo. A linha do tempo da execução é o
    transcript, e o grupo do árbitro põe a linha no bloco dele."""
    try:
        runs = orq_runs.active()
    except Exception:
        _log.warning("orq: leitura das orquestrações falhou (lista segue)", exc_info=True)
        return []
    return [SessionInfo(name=run["name"], cwd=run["repo"], jsonl=run["timeline"], provider="orq",
                        tracked=True, pair_gid=run["gid"], orq_arbiter=run["arbiter"]) for run in runs]


def _decorate_transfers(infos: list[SessionInfo]) -> None:
    from app import conversation_transfer as transfers
    rows = {info.name: info for info in infos}
    for info in infos:
        record = transfers.transfer_for_session(info.name, lifecycle_id=info.lifecycle_id, source_path=info.jsonl)
        if record is None and info.provider == "codex" and info.codex_home:
            meta = codex_sessions.load(info.name)
            if meta and meta.get("thread_id"):
                prepared = transfers.transfer_for_thread(info.codex_home, meta["thread_id"])
                if (prepared and prepared.name == info.name and prepared.phase not in
                        {transfers.TransferPhase.COMPLETE, transfers.TransferPhase.REJECTED,
                         transfers.TransferPhase.ROLLED_BACK}):
                    record = prepared
        if record:
            info.transfer_id, info.transfer_phase = record.id, record.phase.value
    for record in transfers.list_incomplete():
        info = rows.get(record.name)
        if info and info.transfer_id != record.id:
            # Uma sessão recriada com o nome antigo não pertence à recuperação.
            continue
        origin = record.origin_meta
        if info is None:
            info = SessionInfo(name=record.name)
            infos.append(info)
            rows[record.name] = info
        # A fase é apresentação; não inventa um estado físico de turno em execução.
        info.cwd = origin.get("cwd")
        info.jsonl = record.source.path if record.source else origin.get("jsonl")
        info.provider = "claude"
        info.headless = bool(origin.get("headless"))
        info.engine = origin.get("engine")
        info.codex_home = None
        cdir = origin.get("config_dir") or Path.home() / ".claude"
        info.conta = f"claude:{Path(cdir).resolve(strict=False)}"
        info.lifecycle_id = record.source_life
        info.transfer_id, info.transfer_phase = record.id, record.phase.value
        info.problema = record.error_code
        pair = PairLink(record.name).get() or {}
        info.pair_peers, info.pair_gid, info.pair_task = pair.get("peers"), pair.get("gid"), pair.get("task")
        info.pair_external = _pair_external(record.name, pair.get("peers"))
        info.then_target = (ThenLink(record.name).get() or {}).get("target")
        infos[:] = [row for row in infos if row.name != record.name or row is info]


def sanitize_cwd(cwd: str) -> str:
    # O Claude indexa pelo cwd sem separador final ("/home/x/" e "C:\\x\\" caem em "-home-x" e
    # "C--x"); só a raiz ("/", "C:\\") mantém o seu.
    limpo = cwd.rstrip("/\\")
    if not limpo or limpo.endswith(":"):
        limpo = cwd
    return re.sub(r"[^A-Za-z0-9]", "-", limpo)


def cwd_atual(meta: dict) -> str | None:
    """Pasta de uma sessão sem terminal. Renomeada com a sessão viva, o caminho gravado aponta pro
    nada, mas o processo continua dentro dela e o /proc mostra o nome novo. O transcript segue no
    caminho gravado: é por ele que o Claude indexa a conversa."""
    cwd = meta.get("cwd")
    pid = (meta.get("cano") or {}).get("pid")
    chave = meta.get("key") or ""
    if not cwd or os.path.isdir(cwd) or not pid or not chave:
        return cwd
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            if chave[:16].encode() not in fh.read():
                return cwd   # pid reaproveitado por outro processo
        vivo = os.readlink(f"/proc/{pid}/cwd")
    except OSError as e:
        _log.debug("cwd_atual: sem pasta viva de %s (pid %s): %s", meta.get("name"), pid, e)
        return cwd
    return vivo if os.path.isdir(vivo) else cwd


_pretrust_lock = threading.Lock()


def _chave_trust(cwd: str, windows: bool = os.name == "nt") -> str:
    """A chave que o Claude Code usa em `projects` do `.claude.json` para esta pasta.

    No Windows ele normaliza o caminho pra barra NORMAL antes de indexar; gravar com contrabarra
    escreve uma chave que ninguem le, e a sessao nova nascia presa no "trust this folder?" mesmo
    com o pre-trust rodando."""
    return cwd.replace("\\", "/") if windows else cwd


def _claude_service_tier(engine: str | None, model: str | None, tier: str | None) -> str | None:
    if tier is None:
        return None
    if tier not in ("default", "priority"):
        raise ValueError("service_tier: use default ou priority")
    from app import cliproxy
    if cliproxy.supports_fast(engine, model):
        return tier
    if tier == "priority":
        raise ValueError("service_tier exige Claude com motor GPT no CLIProxyAPI local")
    return None


def _env_sessao(modelo: str | None, jev: bool, provider: str = "claude",
                nome: str | None = None, claude_settings: dict | None = None) -> dict:
    env = runtime_config.env_jev(jev)
    env.update(session_customizations.environment(claude_settings if provider == "claude" else None))
    # Quem lê a variável é o binário `claude` — com ou sem motor, que só troca o provedor do modelo.
    # Nos outros providers ela não seria lida por ninguém.
    if provider == "claude":
        env.update(runtime_config.env_function_hooks())
        # Tela cheia sempre: só nela o Claude Code liga o mouse, que é por onde o app aperta os
        # botões dos mods. No Windows por SSH ele a desliga sozinho.
        env["CLAUDE_CODE_NO_FLICKER"] = "1"
        # Endereço e token do caminho nativo de entrada. Só com nome: o pane precisa
        # saber por qual sessão ele responde, e é o nome que a fila usa. A chave é deste
        # processo: o `resolve_binding` a lê do ambiente dele e a leva ao servidor Rust.
        if nome:
            env.update(plugin_bridge.env_da_sessao(nome, secrets.token_hex(16)))
    if modelo:
        env["CLAUDE_CODE_SUBAGENT_MODEL"] = modelo
    return {"env": env}


def _jev_do_processo(pid: int | None) -> bool:
    """O relancamento (troca de modelo, resume) le o estado do processo que vai morrer — mesma
    tecnica do CLAUDE_CODE_SUBAGENT_MODEL. A CHAVE nao e copiada: e relida do runtime_config, pra
    sessao ressuscitada nao ficar presa numa chave que ja foi trocada."""
    return bool(pid) and procinfo._env_var_of(pid, runtime_config.MARCA_JEV) == "on"


def _esperar_saida(pids: list[int], teto_s: float = 5.0) -> None:
    fim = time.monotonic() + teto_s
    while any(procinfo.pid_vivo(p) for p in pids):
        if time.monotonic() >= fim:
            _log.warning("troca de modo: processo(s) %s seguem vivos apos %.0fs", pids, teto_s)
            return
        time.sleep(0.1)


def _pretrust_cwd(cwd: str, config_dir: str | None) -> None:
    """Marca `hasTrustDialogAccepted=True` pra `cwd` no .claude.json que a sessão nova vai LER —
    quem responde qual é o arquivo é `tmux.claude_json_de`, o mesmo lugar que decide se o pane
    recebe `CLAUDE_CONFIG_DIR` (duas cópias da regra é como o pre-trust escrevia no arquivo errado
    e a sessão nascia presa no "trust this folder?" mesmo assim).
    Pré-aprova o dialog que o Claude Code mostra no 1º acesso a uma
    pasta nova. Read-modify-write atômico (tmp+replace) sob _pretrust_lock: dois create()
    concorrentes (rodam em threads via to_thread) fariam read-modify-write no MESMO arquivo e
    last-write-wins perderia uma entrada — mesmo padrão do _append_lock (pqueue) / _LOCK (pair).
    ensure_ascii=False + indent=2: NÃO reserializa chaves com acento pra \\uXXXX nem colapsa o
    arquivo pretty-printed do usuário (reescreve o dict inteiro; preserva o formato).
    Best-effort — qualquer falha só deixa o dialog aparecer, como hoje.
    JANELA RESIDUAL: o _lock é intra-processo; se o PRÓPRIO Claude Code CLI (processo externo)
    reescrever o .claude.json entre nosso read e replace, essa escrita dele é perdida (last-write-
    wins). Janela ~ms, o CLI escreve raro, e o guard 'já confiada -> return' limita a 1x/pasta ->
    colisão rara e aceita; fechar exigiria flock que o CLI teria de respeitar (não verificável)."""
    with _pretrust_lock:
        try:
            cfg = tmux.claude_json_de(config_dir)
            data = json.loads(cfg.read_text(encoding="utf-8")) if cfg.exists() else {}
            projects = data.setdefault("projects", {})
            entry = projects.setdefault(_chave_trust(cwd), {})
            if entry.get("hasTrustDialogAccepted") is True:
                return  # já confiada -> não reescreve o arquivo (evita corrida à toa)
            entry["hasTrustDialogAccepted"] = True
            tmp = cfg.with_suffix(".json.cp-tmp")
            tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
            atomico.substituir(tmp, cfg)
        except Exception as e:
            _log.warning("pretrust falhou pra %s: %r", cwd, e)



def _newest_after_clear(projdir: Path, sid_jsonl: str, exclude: set[str]) -> str:
    # /clear rola um session-id NOVO (novo .jsonl) sem alterar o --session-id do cmdline -> o jsonl do
    # cmdline congela no transcript de BOOT. Se o projeto tem um .jsonl mais recente (e nao seguro por um
    # subagente/daemon), ele e o transcript pos-clear do mesmo REPL: segue ele. Senao devolve sid_jsonl.
    # ponytail: heuristica por mtime no mesmo cwd. Teto: durante uma Task, se o subagente escrever por
    # ultimo SEM estar com fd aberto no instante (abre/escreve/fecha), pode pegar o jsonl dele num poll
    # -> transitorio, o REPL reassume ao gravar a resposta. Upgrade: o REPL marcar seu transcript ativo
    # explicitamente (ex: hook gravando o path).
    try:
        best_mt = os.path.getmtime(sid_jsonl)
    except OSError:
        # boot-id ainda nao escrito (sessao recem-criada) -> sem /clear possivel ainda; confia no
        # --session-id deterministico (o tailer segue quando o arquivo aparecer). NAO cair pro mtime aqui
        # senao um jsonl antigo do mesmo cwd venceria o transcript novo que ainda nem nasceu.
        return sid_jsonl
    best = sid_jsonl
    try:
        for f in projdir.glob("*.jsonl"):
            if os.path.realpath(str(f)) in exclude:
                continue
            try:
                mt = f.stat().st_mtime
            except OSError:
                continue
            if mt > best_mt:
                best, best_mt = str(f), mt
    except OSError:
        pass
    return best


# pasta .hangar-active -> {nome: (mtime_ns, jsonl, pid, ts)}
_marker_cache: dict[str, dict[str, tuple[int, Optional[str], object, float]]] = {}


def _marker_by_pids(config_base: Path, pids: list[int], exclude: set[str]) -> Optional[str]:
    # Marcador do hook casado por PID: o state_hook grava {jsonl, ts, cwd, pid} onde pid = o REPL
    # claude que disparou o evento. Se esse pid e DESCENDENTE deste pane, o marcador e desta sessao
    # — resolve sessao BARE (sem --session-id no cmdline) de forma deterministica, sem chute por
    # mtime. Varios marcadores casando (ex: restart do claude no mesmo pane) -> o mais recente vence.
    d = config_base / ".hangar-active"
    pidset = set(pids)
    best: tuple[float, str] | None = None
    try:
        entries = list(os.scandir(d))
    except OSError:
        return None
    # Relido so o marcador cujo mtime mudou: com centenas deles, o json.loads de todos a cada tick
    # da lista era o custo. O dict e refeito por chamada, entao marcador apagado sai dele.
    anterior = _marker_cache.get(str(d), {})
    atual: dict[str, tuple[int, Optional[str], object, float]] = {}
    for e in entries:
        if not e.name.endswith(".json"):
            continue
        try:
            mtime = e.stat().st_mtime_ns
        except OSError:
            continue
        hit = anterior.get(e.name)
        if hit is None or hit[0] != mtime:
            try:
                with open(e.path, encoding="utf-8") as fh:
                    o = json.loads(fh.read())
                hit = (mtime, o.get("jsonl"), o.get("pid"), float(o.get("ts") or 0.0))
            except (OSError, ValueError, AttributeError, TypeError):
                continue
        atual[e.name] = hit
    _marker_cache[str(d)] = atual
    for _mt, j, pid, ts in atual.values():
        if not j or pid not in pidset:
            continue
        if not os.path.exists(j) or os.path.realpath(j) in exclude:
            continue
        if best is None or ts > best[0]:
            best = (ts, j)
    return best[1] if best else None


def _active_marker_jsonl(config_base: Path, sid: str, exclude: set[str]) -> Optional[str]:
    # Marcador do hook (state_hook.py): <config>/.hangar-active/<boot_id>.json = {"jsonl": <path>}
    # = o transcript REALMENTE ativo daquele boot_id. Sinal DETERMINISTICO pro caso resume/clear, onde
    # o <boot_id>.jsonl do cmdline NUNCA nasce (o claude escreve no <uuid> resumido) -> sem isto resolvia
    # pro path fantasma = chat vazio. So vale se o arquivo existe e nao e de um auxiliar (subagente/daemon).
    p = config_base / ".hangar-active" / f"{sid}.json"
    try:
        j = json.loads(p.read_text(encoding="utf-8")).get("jsonl")
    except (OSError, ValueError):
        return None
    if not j or not os.path.exists(j) or os.path.realpath(j) in exclude:
        return None
    return j


# session-id (uuid) na linha de comando do claude: `--session-id <uuid>` / `--session-id=<uuid>` /
# `--resume <uuid>`. Este e o sinal AUTORITATIVO e ESTAVEL (vive no /proc/PID/cmdline pela vida do
# processo, inclusive em idle) -> o jsonl da sessao e <uuid>.jsonl. So casa uuid de verdade pra nao
# pescar argumento de outra flag.
_SID_RE = re.compile(
    r"--(?:session-id|resume)[ =]"
    r"([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})"
)


def _session_id_from_cmdline(cmdline: str) -> Optional[str]:
    m = _SID_RE.search(cmdline)
    return m.group(1) if m else None


def _jsonl_mtime(jsonl: Optional[str]) -> Optional[float]:
    # last_activity = mtime do transcript (epoch s). Usado pro desempate da ordenacao na lista.
    if not jsonl:
        return None
    try:
        return os.path.getmtime(jsonl)
    except OSError:
        return None


def _kimi_corrige_ocioso(info, marker):
    """Aplica `corrige_ocioso_kimi` (app/state.py) a uma linha da lista. So pra provider kimi."""
    if getattr(info, "provider", "claude") != "kimi":
        return marker
    return corrige_ocioso_kimi(marker, info.jsonl)


# Executavel do agente -> provider. Casa o BASENAME do argv[0], nunca a linha inteira: `pip`,
# `pipx`, `mpirun` e um caminho contendo "/pi/" nao sao o agente Pi. O Kimi entra DUAS vezes:
# sessões antigas (0.36.x) reescrevem o argv0 como `kimi`; a 0.37.2 reescreve como `kimi-code` —
# sem a segunda entrada, a sessão recém-criada virava "claude" na re-descoberta e herdava até o
# transcript do Claude do mesmo cwd (medido no e2e de 19/08/2026).
# O `codex` entra por causa da JANELA entre o pane nascer e o sidecar existir: a TUI e o app-server
# sobem juntos pelo lancador, e ate a thread abrir nao ha sidecar nenhum. Sem esta linha o pane cai
# no default "claude" e e casado com o transcript do CLAUDE do mesmo diretorio — a mesma regressao
# que ja custou caro no Pi.
# O `omp` entra por SI: o binario do oh-my-pi e um ELF nativo com argv0 `omp`, nao um fork do
# processo `pi` -- sem esta entrada o pane cai no default "claude" e e casado com o transcript do
# Claude do mesmo cwd, a mesma regressao do Pi acima.
_EXEC_PROVIDER = {"pi": "pi", "omp": "omp", "claude": "claude", "kimi": "kimi", "kimi-code": "kimi",
                  "codex": "codex", "hangar-codex-tui": "codex"}

# Windows: o argv0 vem com extensao (`claude.exe`), que nao casa em _EXEC_PROVIDER; e um CLI
# instalado por `npm -g` nao aparece com o nome dele nenhuma vez — o processo e o
# `node.exe <...>\<pacote>\dist\cli.js`. Medido nesta VM em 21/08/2026, com o pi 0.84.2 aberto num
# pane: os descendentes eram `powershell.exe -NoLogo -Command pi` e
# `node.exe C:\...\npm/node_modules/@earendil-works/pi-coding-agent/dist/cli.js`. Nenhum dos dois
# casava, entao TODA sessao Pi era classificada como Claude — e dai o app nem procurava o bilhete
# da extensao, resolvia um jsonl do layout do Claude e caia no scrape do pane pra statusline.
# So o que foi MEDIDO entra aqui: kimi e codex no Windows ainda nao foram verificados, e chutar o
# nome de pacote deles seria pior que a ausencia (viraria deteccao errada em vez de nenhuma).
_PKG_PROVIDER = {"pi-coding-agent": "pi"}


def _exigir_cp_engine() -> None:
    """Recusa ALTO quando o `hangar-engine` nao esta no PATH. Chamado antes de montar o prefixo.

    O `hangar-engine --exec` vira o COMANDO do pane. Sem o lancador no PATH o pane morre no ato — e o
    `tmux new-session` devolve 0 do mesmo jeito. Medido nesta VM (psmux 3.3.7): rc=0 na criacao e,
    tres segundos depois, `has-session` ja responde 1. Sem esta guarda o backend responde "sessao
    criada" pro celular e a sessao some sem deixar rastro, que e o pior modo de falha que existe
    aqui — o usuario nao tem nem o que procurar.

    Conferir ANTES em vez de verificar DEPOIS e deliberado: "o pane sobreviveu?" e uma corrida (ele
    pode morrer a qualquer instante depois da checagem), enquanto "o lancador existe?" e uma
    pergunta estavel, e a resposta ja diz o que fazer.

    RESSALVA: o PATH consultado e o do BACKEND, e o comando roda no PATH do PANE. Onde os dois
    divergirem isto pode recusar uma criacao que funcionaria. Recusa visivel e com instrucao e
    melhor que sessao que evapora calada, e o conserto (instalar o lancador) serve pros dois.
    """
    if shutil.which("hangar-engine"):
        return
    raise ValueError(
        "hangar-engine nao esta no PATH deste servidor — sem ele a sessao com motor nasce e morre na "
        "hora, sem erro. Instale o lancador: no Linux, scripts/install-claude-wrapper.sh; no "
        "Windows, install.ps1.")


def _exigir_lancador_codex() -> None:
    """Mesma guarda do `_exigir_cp_engine`, pro lancador do Codex — e pelo mesmo motivo exato.

    `hangar-codex-tui` e o COMANDO do pane de toda sessao Codex. Faltando no PATH, o pane morre no
    ato e o `tmux new-session` devolve 0: o app diria "sessao criada" e a sessao sumiria sem rastro.
    """
    from app.adapters.codex.lancador import EXECUTAVEL
    if not shutil.which(EXECUTAVEL):
        raise ValueError(
            f"{EXECUTAVEL} nao esta no PATH deste servidor — sem ele a sessao Codex nasce e morre "
            "na hora, sem erro. Instale o lancador: no Linux, scripts/install-claude-wrapper.sh; "
            "no Windows, install.ps1.")
    # O `codex` tambem: o lancador so o chama DEPOIS que o pane ja nasceu, entao a falta dele nao
    # cai aqui — cai num `FileNotFoundError` dentro do pane, que morre no ato com o `new-session`
    # ja tendo devolvido 0. Mesmo desfecho da linha acima (sucesso reportado, sessao inexistente),
    # e por isso a mesma guarda: alto e antes de criar nada.
    if not shutil.which("codex"):
        raise ValueError(
            "codex nao esta no PATH deste servidor — o lancador sobe o app-server com ele, e sem "
            "isso a sessao Codex nasce e morre na hora, sem erro. Instale o CLI do Codex.")


def _provider_do_argv(argv: list[str]) -> Optional[str]:
    """Provider a partir do argv JA separado, ou None. Nao recebe string: ver procinfo._argv."""
    if not argv:
        return None
    base = os.path.basename(argv[0])
    if os.name == "nt":
        base = os.path.splitext(base)[0]      # claude.exe -> claude
    prov = _EXEC_PROVIDER.get(base)
    if prov:
        return prov
    # A integração roda no lançador antes de existir qualquer processo `codex`.
    if re.fullmatch(r"python(?:\d+(?:\.\d+)*)?(?:\.exe)?", base) and len(argv) > 1:
        if argv[1].replace("\\", "/").rsplit("/", 1)[-1] == "hangar-codex-tui":
            return "codex"
    # Lancado por node: quem diz qual agente e o CAMINHO do script, nao o interpretador. Normaliza
    # a barra porque o npm do Windows monta o shim com `/` no meio de um caminho com `\`.
    if base in ("node", "node.exe"):
        for arg in argv[1:]:
            partes = arg.replace("\\", "/").split("/")
            for pkg, p in _PKG_PROVIDER.items():
                if pkg in partes:
                    return p
    return None


def agente_do_pane(pid, children: Optional[dict[int, list[int]]] = None) -> tuple[str, Optional[int]]:
    """Qual agente roda neste pane e QUAL pid e o dele, lidos do /proc dos descendentes.

    NAO ha campo de comando no pane: tmux.list_panes_active() devolve so name/pid/cwd/pane_id, entao
    o caminho e o mesmo do _repl_sid — descer os descendentes e ler o cmdline.

    O pid importa porque `CLAUDE_CONFIG_DIR`/`CP_ENGINE` moram no ambiente do processo do AGENTE. Numa
    sessao aberta a mao o pane e o shell, que nao declara nenhum dos dois: lendo o pid do pane, a conta
    caia no default (`~/.claude`) e a sessao aparecia com o badge da conta errada.

    Default "claude" preserva o comportamento anterior a esta funcao existir: pane nao reconhecido
    segue tratado como Claude, em vez de sumir da lista. Pid None = nao achou agente; o call site
    decide o fallback.
    """
    if not pid:
        # pid 0 (System Idle no Windows) e pai dele mesmo no mapa do psutil e tem a arvore da maquina inteira embaixo — visitar ele custa um _cmdline por processo da maquina, a cada poll
        return "claude", None
    for p in _descendant_pids(pid, children):
        cmd = _cmdline(p)
        if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
            continue        # mesma exclusao do _repl_sid: subprocesso nao e o REPL dono
        # _argv e nao `cmd.split()`: no Windows o caminho do executavel tem espaco
        # (`C:\Program Files\nodejs\node.exe`) e o split devolvia `C:\Program` como argv0. O `cmd`
        # acima segue servindo pra exclusao por substring, que e o uso dele.
        prov = _provider_do_argv(_argv(p))
        if prov:
            return prov, p
    return "claude", None


def provider_of_pane(pid, children: Optional[dict[int, list[int]]] = None) -> str:
    return agente_do_pane(pid, children)[0]


def _esforco_de_abertura(esforco):
    """Esforço em uso que a flag `--effort` aceita; `ultracode` só existe no `/effort` em voo."""
    return esforco if esforco in model_args.EFFORT_CLAUDE else None


def _pid_do_agente(pane_pid):
    """Pid de onde ler conta, motor e modelo: o do agente dentro do pane, ou o próprio pane."""
    return (agente_do_pane(pane_pid)[1] or pane_pid) if pane_pid else None


# Cache pid -> (instante de inicio do processo, nome da sessao tmux). Um processo nunca muda de
# sessao (nasce dentro do pane), entao a resposta e estavel enquanto ELE vive. A chave carrega o
# start time porque o pid sozinho NAO identifica processo: reusado depois que o dono morreu (churn
# alto, pid_max baixo, container), a entrada velha seria devolvida pro processo NOVO e o recado
# apareceria vindo da sessao errada, calado — o comentario anterior afirmava que isso nao acontecia,
# e o codigo nao fazia o que ele dizia (achado da revisao). Guarda tambem o resultado vazio: sem
# isso, recado de sessao fora do tmux (um `claude -p` solto) pagaria um fork de tmux por linha.
# EXCECAO, e ela e real: onde `/proc/<pid>/stat` nao da pra ler (hidepid=2, backend sob outro uid),
# o start time e None SEMPRE e o cache — inclusive o negativo — nunca vale. Ali o fork por linha
# volta, e um `GET /history` sem limite (que reparseia o jsonl inteiro) paga um por recado. E o
# preco de nao arriscar atribuir recado a sessao errada; se doer, o conserto e cachear por
# (pid, jsonl do remetente), nao afrouxar a chave.
_NOME_POR_PID: dict[int, tuple[Optional[float], Optional[str]]] = {}


def name_of_pid(pid: int) -> Optional[str]:
    """Nome da sessao tmux dona deste pid, ou None.

    Existe pro recado nativo entre sessoes Claude (cross-session messaging): o transcript do destino
    traz `origin.verifiedPeerPid` do REMETENTE, e o app precisa do nome tmux — que e o endereco que
    o hangar-send, o pareamento e a UI usam. O `origin.name` que vem junto NAO serve: e o titulo da
    sessao ("Revisar novo modo de envio no backlog"), nao o nome (medido em 07/08/2026).

    Roda no parse do transcript, que vive num `to_thread` — o fork do tmux aqui nao toca o laco de
    eventos. Falha (tmux fora, timeout) devolve None: quem chama tem fallback, e recado sem nome
    resolvido e melhor que transcript que para de ser lido.
    """
    nascimento = _proc_start_time(pid)
    # `nascimento is not None` faz parte da condicao: sem ele, um ambiente onde o /proc/<pid>/stat
    # nao da pra ler (hidepid=2, backend sob outro uid — o mesmo cenario que o inbox_socket_of ja
    # reconhece) devolve None SEMPRE, `None == None` casa, e o cache volta a ser por pid puro — o
    # bug de atribuicao errada que esta chave existe pra fechar, de volta pela porta dos fundos
    # (achado da revisao do proprio conserto). Nao saber a idade do processo = nao confiar no cache.
    if (cache := _NOME_POR_PID.get(pid)) is not None and nascimento is not None \
            and cache[0] == nascimento:
        return cache[1]
    achado: Optional[str] = None
    try:
        children = _proc_children_map()
        for nome, panes in tmux.list_panes_all().items():
            for pane in panes:
                ppid = pane.get("pid")
                if ppid and (ppid == pid or pid in _descendant_pids(ppid, children)):
                    achado = nome
                    break
            if achado:
                break
    except Exception:                                # noqa: BLE001
        _log.warning("name_of_pid(%d) falhou; recado fica com o nome do remetente", pid,
                     exc_info=True)
        return None                                  # NAO cacheia falha: a proxima tentativa retenta
    _NOME_POR_PID[pid] = (nascimento, achado)
    return achado


def inbox_socket_of(name: str) -> Optional[str]:
    """Socket de inbox do cross-session messaging desta sessao, ou None se ela nao tem.

    O Claude Code (2.1.224+) liga um socket por sessao em `$XDG_RUNTIME_DIR/cc-socks/<pid>.sock`
    (medido em 07/08/2026; o pid e o do processo `claude`). Ter o socket e o que torna a sessao
    alcancavel pelo `SendMessage` de outra — e o que o `ListAgents` de la vai listar.

    Serve pro hangar-send decidir, com FATO em vez de suposicao, se o caminho nativo existe pra este
    alvo: sessao aberta antes da liberacao, sessao Codex/Pi ou sessao de outra maquina nao tem
    socket nenhum, e mandar o modelo usar `SendMessage` nesses casos seria mandar ele bater numa
    porta que nao existe.
    """
    # Gate de plataforma ANTES de qualquer coisa: `os.getuid()` nao existe no Windows, e e justamente
    # la que XDG_RUNTIME_DIR costuma faltar — o AttributeError subia direto pra rota /peer-address e
    # ela dava 500 em TODA chamada naquela plataforma (achado da revisao). O Claude Code tambem nao
    # oferece a feature no Windows nativo, entao "sem socket" e a resposta certa, nao um erro.
    if os.name != "posix":
        return None
    try:
        # O `is_dir()` tambem fica DENTRO do try: ele engole ENOENT/ENOTDIR mas RELEVANTA EACCES, e
        # um XDG_RUNTIME_DIR sem permissao (backend sob outro uid, escopo do systemd mais apertado,
        # container com namespace proprio) mandava PermissionError direto pra rota /peer-address —
        # 500 em toda chamada naquele ambiente. Mesma classe do gate de Windows acima, um passo antes
        # (achado da revisao).
        run = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
        socks = Path(run) / "cc-socks"
        if not socks.is_dir():
            return None                              # feature ausente/desligada nesta maquina
        pane = tmux.pane_pid(name)
        if not pane:
            return None
        for p in _descendant_pids(pane, _proc_children_map()):
            caminho = socks / f"{p}.sock"
            if caminho.exists():
                return str(caminho)
    except Exception:                                # noqa: BLE001
        _log.warning("inbox_socket_of(%r) falhou; tratando como sem socket", name, exc_info=True)
    return None


def _pi_sid_of(pid: int) -> Optional[str]:
    # CP_PI_SESSION: o uuid que o wrapper do pi injetou. Mesmo truque do _engine_of — o env do
    # processo VIVO e o registro autoritativo. Existe porque o `--session-id` some do cmdline: o pi
    # sobrescreve o proprio argv (medido na Task 0).
    if not procinfo._TEM_PROC:
        # Escapou do procinfo quando ele foi extraido: sem /proc isto abria um caminho de arquivo
        # inexistente e caia no `except OSError: return None`, perdendo o fallback CP_PI_SESSION —
        # a sessao Pi entrava na lista sem transcript, em silencio.
        return procinfo._env_psutil(pid).get("CP_PI_SESSION") or None
    try:
        with open(procinfo._proc_environ_path(pid), "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(b"CP_PI_SESSION="):
                    return kv.split(b"=", 1)[1].decode("utf-8", "replace") or None
    except OSError:
        return None
    return None


def _pi_transcript_of_id(cwd: str, sid: str, provider: str = "pi", perfil: str | None = None) -> Optional[str]:
    # Indireção pro adapter (Task 1), que sabe o slug e o glob <timestamp>_<uuid>.jsonl. Import local
    # pelo mesmo motivo do get_adapter em create(): evita qualquer ciclo se um adapter futuro vier a
    # importar daqui.
    from app.adapters import get_adapter
    return get_adapter(provider).transcript_path(cwd, sid, perfil) or None


def _omp_profile_of(pid: Optional[int]) -> Optional[str]:
    # Perfil do omp DAQUELE pane: move a raiz das sessoes pra ~/.omp/profiles/<p>/agent. Lido do
    # processo vivo, como CP_ENGINE e CLAUDE_CONFIG_DIR — o backend pode estar noutro perfil.
    return procinfo._env_var_of(pid, "OMP_PROFILE") if pid else None


def _pi_is_subagent(path: str) -> bool:
    # Import local pelo mesmo motivo do _pi_transcript_of_id. Quem sabe o layout no disco e o
    # adapter; aqui so se decide o que fazer com a resposta.
    from app.adapters.pi.sessions import is_subagent_transcript
    return is_subagent_transcript(path)


def _pi_root_transcript(path: str) -> Optional[str]:
    from app.adapters.pi.sessions import root_transcript
    return root_transcript(path) or None


# Panes ja avisados sobre bilhete sem frescor. list() e polled (de segundo em segundo), entao um
# warning por varredura entupiria o journal; um por pane+motivo basta pra um /proc cronicamente
# ilegivel nao ficar calado pra sempre. ponytail: set simples — o teto e o numero de panes da
# maquina, nao ha o que expirar.
_PI_TICKET_WARNED: set[tuple[str, str]] = set()


def _warn_bilhete_once(pane_id: str, motivo: str) -> None:
    if (pane_id, motivo) not in _PI_TICKET_WARNED:
        _PI_TICKET_WARNED.add((pane_id, motivo))
        _log.warning("pi: bilhete de %s recusado (%s); usando CP_PI_SESSION", pane_id, motivo)


def _chave_do_bilhete(pane_id: str, pid: Optional[int]) -> str:
    """Mesma chave que a extensao usa pra gravar o bilhete (scripts/pi/hangar-state.ts, paneKey).

    No tmux e o `%N`, que e unico no servidor. No psmux (Windows) NAO e: medido em 21/08/2026,
    quatro sessoes vivas ao mesmo tempo e todas com `TMUX_PANE=%1` — a segunda sessao Pi
    sobrescrevia o bilhete da primeira e `pi_session_file` passava a devolver o MESMO transcript
    pras duas, ou seja, uma abria a conversa da outra. Ali a chave e o `PSMUX_SESSION`, que carrega
    o nome da sessao e e unico por construcao.

    Lido do ambiente do PROCESSO do pane, e nao de um parametro novo, pra ser exatamente o que a
    extensao leu — as duas pontas olham a mesma fonte, entao nao ha como divergirem. No Linux a
    variavel nao existe, `_env_var_of` devolve None e a chave fica byte-identica a de sempre.
    """
    if pid is not None:
        psmux = procinfo._env_var_of(pid, "PSMUX_SESSION")
        if psmux:
            return re.sub(r"[^A-Za-z0-9._-]", "-", psmux)
    return pane_id.lstrip("%")


def pi_session_file(pane_id: str, pid: Optional[int] = None,
                    cwd: str = "", provider: str = "pi") -> Optional[str]:
    """Transcript de um pane Pi: bilhete da extensao primeiro, env do wrapper depois.

    Nenhum dos dois presente -> None, e a sessao entra na lista SEM transcript. Chutar o arquivo
    mais novo do cwd (o que resolve_jsonl faz pro Claude) faria a sessao Pi abrir mostrando a
    conversa de outro agente.
    """
    base = (_config_dir_of(pid) if pid else None) or Path.home() / ".claude"
    ticket = Path(base) / ".hangar-pi" / f"{_chave_do_bilhete(pane_id, pid)}.json"
    sid = _pi_sid_of(pid) if pid else None
    try:
        # encoding explicito: o bilhete guarda o CAMINHO do transcript, e no Windows o default e
        # cp1252. Uma pasta com acento (Area de trabalho) ou emoji volta corrompida daqui — e o
        # caminho corrompido nao existe, entao a sessao Pi ficaria sem transcript sem erro nenhum.
        data = json.loads(ticket.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            return None            # ver o irmao em kimi_session_file: nao-dict derruba list() inteira
        f, ts = data.get("file"), data.get("ts")
        # Bilhete de OUTRA encarnacao do pane: o tmux reusa %pane_id apos um restart do servidor e o
        # .jsonl da sessao anterior continua no disco, entao o exists() abaixo nao pega nada — o pane
        # novo abriria a conversa do pane velho. O criterio e FRESCOR, nunca "os ids divergem": a
        # extensao reescreve o bilhete a cada agent_start justamente porque /tree, /fork e troca de
        # sessao mudam o arquivo com a sessao ja rodando, enquanto o CP_PI_SESSION fica congelado no
        # /proc desde o exec. Depois de um fork a divergencia e o comportamento CORRETO; rejeitar por
        # ela devolveria a conversa anterior pelo resto da vida do pane. Bilhete escrito ANTES de o
        # processo deste pane nascer e que e de outra sessao.
        nasceu = _proc_start_time(pid) if pid else None
        # 2s de folga: o ts vem do Date.now() da extensao e o nascimento, do btime+ticks do kernel —
        # granularidades e relogios diferentes, e o bilhete do session_start nasce colado no exec.
        if nasceu is None or not isinstance(ts, (int, float)):
            # Frescor INDETERMINAVEL: /proc/<pid>/stat ilegivel (pid morto, permissao, kernel sem
            # /proc) ou bilhete sem `ts` numerico (extensao antiga, escrita parcial). Recusa, igual a
            # um bilhete velho — deixar passar era o furo silencioso: o guarda simplesmente nao
            # rodava e o pane reusado abria a conversa da encarnacao ANTERIOR, tracked=True e sem
            # nenhum rastro. Sem frescor o CP_PI_SESSION e o unico sinal que ainda prova de quem e
            # o pane (vem do /proc do processo VIVO).
            _warn_bilhete_once(pane_id, "nascimento" if nasceu is None else "ts")
            f = None
        elif ts < nasceu - 2:
            f = None
        elif f and _pi_is_subagent(f):
            # O Pi dispara `agent_start` TAMBEM pro subagente (Task tool), com um ctx cujo
            # getSessionFile() aponta pro transcript do subagente — e o publishPane da extensao
            # reescreve o bilhete com ele. Aceitar isso trocava a conversa inteira da sessao pela do
            # subagente no app (medido 2026-07-30, numa sessão real: bilhete do pane %26 caiu em
            # `…_18e48e08-…/44bad0fb/run-2/session.jsonl`), enquanto o terminal seguia normal — ele
            # nao le o bilhete. Tratar aqui e mais forte que consertar so a extensao: pega TODA
            # sessao Pi ja de pe, sem reinstalar nem reiniciar nada. O caminho do subagente carrega
            # a raiz dentro dele, entao subimos pra ela em vez de devolver None e deixar a sessao
            # sem transcript ate o proximo turno do agente principal reescrever o bilhete.
            _warn_bilhete_once(pane_id, "subagente")
            f = _pi_root_transcript(f)
        # Bilhete FRESCO vale mesmo com o arquivo ainda inexistente: o Pi so escreve o .jsonl no 1o
        # turno, e a extensao publica o bilhete la no session_start. Exigir exists() aqui deixava
        # TODA sessao Pi recem-criada pelo app como "sem id" — sem transcript e inclicavel — ate
        # alguem digitar a primeira mensagem no terminal, que e justamente o que nao da pra fazer
        # pelo celular. O caso que o exists() guardava (bilhete orfao de uma encarnacao anterior do
        # pane, apontando pra .jsonl deletado) ja e coberto pelo teste de frescor acima, que e mais
        # forte: compara o bilhete com o nascimento DESTE processo. Mesmo contrato do Claude, cujo
        # create() tambem fixa um caminho que so passa a existir depois.
        if f:
            # omp: o bilhete devolve o caminho do `--session`, mas o transcript principal nasce
            # em `sessions/-/<nome>` (ver pi_sessions.localizar_na_raiz). Mesmo nome, outra pasta.
            if provider == "omp" and not os.path.exists(f):
                from app.adapters.pi.sessions import localizar_na_raiz   # import local, como os irmaos acima
                f = localizar_na_raiz(os.path.basename(f), provider, _omp_profile_of(pid)) or f
            return f
    except (OSError, ValueError):
        pass
    if not sid:
        return None
    perfil = _omp_profile_of(pid) if provider == "omp" else None
    return _pi_transcript_of_id(cwd, sid, provider, perfil) if perfil else _pi_transcript_of_id(cwd, sid, provider)


_KIMI_TICKET_WARNED: set[tuple[str, str]] = set()


def _warn_kimi_once(pane_id: str, motivo: str) -> None:
    # Mesmo aviso-uma-vez do _warn_bilhete_once: list() e polled, um warning por varredura
    # entupiria o journal.
    if (pane_id, motivo) not in _KIMI_TICKET_WARNED:
        _KIMI_TICKET_WARNED.add((pane_id, motivo))
        _log.warning("kimi: bilhete de %s recusado (%s)", pane_id, motivo)


def kimi_session_file(pane_id: str, pid: Optional[int] = None,
                      cwd: str = "") -> Optional[str]:
    """Transcript (wire.jsonl) de um pane Kimi: so o bilhete do hook — NAO ha fallback por env
    (o wrapper do kimi nao injeta id; o CLI nao aceita id escolhido pelo caller).

    Bilhete ausente/velho -> None, e a sessao entra na lista SEM transcript (tracked=False),
    nunca um chute newest-by-mtime que abriria o wire de OUTRA sessao. E temporario por
    construcao: o hook grava o bilhete no 1o evento da sessao (SessionStart/UserPromptSubmit)
    e a proxima varredura resolve. Mesmo contrato do pi_session_file, inclusive o teste de
    FRESCOR: o tmux reusa %pane_id apos restart do servidor, e um bilhete da encarnacao anterior
    apontaria pra sessao errada.
    """
    base = (_config_dir_of(pid) if pid else None) or Path.home() / ".claude"
    # Mesma chave do bilhete do Pi, pelo mesmo motivo: no psmux o %N nao e unico e dois panes Kimi
    # dividiriam um bilhete so (ver _chave_do_bilhete).
    ticket = Path(base) / ".hangar-kimi" / f"{_chave_do_bilhete(pane_id, pid)}.json"
    try:
        data = json.loads(ticket.read_text(encoding="utf-8"))   # mesmo motivo do pi_session_file
        if not isinstance(data, dict):
            # JSON VALIDO do tipo errado (`null`, lista) nao levanta ValueError, entao o except
            # abaixo nao pega: `.get` num nao-dict e AttributeError, que sobe pelo loop de list()
            # SEM guarda e apaga TODAS as sessoes da tela (Claude, Codex, Pi junto). Mesmo furo que
            # o CLAUDE.md ja registra pro hook_state. Bilhete torto acontece: o _write_marker do
            # hook usa tmp de nome fixo, sem pid, entao dois eventos sobrepostos entrelacam bytes.
            return None
        sid, ts = data.get("session_id"), data.get("ts")
        if not sid:
            return None
        nasceu = _proc_start_time(pid) if pid else None
        if nasceu is None or not isinstance(ts, (int, float)):
            # Frescor INDETERMINAVEL (proc ilegivel ou bilhete sem ts): recusa, como no Pi —
            # deixar passar era o furo silencioso do pane reusado abrindo a conversa anterior.
            _warn_kimi_once(pane_id, "nascimento" if nasceu is None else "ts")
            return None
        if ts < nasceu - 2:
            # 2s de folga: relogios/granularidades diferentes (mesmo criterio do bilhete do Pi).
            return None
        from app.adapters.kimi import sessions as kimi_sessions
        # cwd do bilhete (o hook grava o cwd que o CLI reporta) com fallback pro do pane.
        wire = kimi_sessions.transcript_path(data.get("cwd") or cwd, sid)
        return wire or None
    except (OSError, ValueError):
        return None


# Cadencia do cache de statusline da lista (list_with_state): TTL por sessao + teto de capturas de
# pane por chamada (o custo real e o fork do tmux).
def _claude_config_dir(info) -> Optional[str]:
    """Pasta de configuração da conta da sessão Claude (`conta` = "claude:<pasta>")."""
    conta = getattr(info, "conta", None) or ""
    return conta.split(":", 1)[1] if conta.startswith("claude:") else None


def _claude_context(info, pid: Optional[int]) -> Optional[dict]:
    """Contexto pelo transcript com o modelo e a janela da PRÓPRIA sessão quando se sabe: o
    `/model` que a statusline recebeu, o `--model` do processo ou o sidecar da sem terminal."""
    return _claude_reading(info, pid)[0]


def _claude_reading(info, pid: Optional[int]) -> tuple[Optional[dict], Optional[str]]:
    """Contexto e modelo em uso da sessão Claude, numa leitura só do transcript."""
    model, declared = _escolhas_status(Path(info.jsonl).stem)[0], None
    if getattr(info, "headless", False):
        meta = headless_sessions.load(info.name) or {}
        model, declared = model or meta.get("model"), meta.get("context_window")
    elif pid:
        model = model or procinfo._model_of(pid)[0]
        declared = procinfo._env_var_of(pid, "CLAUDE_CODE_MAX_CONTEXT_TOKENS")
    window = int(declared) if declared and str(declared).isdigit() else None
    config_dir = _claude_config_dir(info)
    ctx, answered = claude_context.read(info.jsonl, config_dir, model, window)
    return ctx, claude_context.session_model(
        answered, model, config_dir, (ctx or {}).get("used", 0), bool(getattr(info, "engine", None)))


_STATUS_TTL = 20.0
_STATUS_BUDGET = 2

apos_saida_codex: Optional[Callable[[str], None]] = None
apos_renomear_codex: Optional[Callable[[str, str], None]] = None


class KillFailed(Exception):
    """A sessao continuou de pe depois do kill. Existe pra a rota DELETE reportar em vez de responder
    {"ok": true} e a UI sumir com um card de sessao que segue viva (ver SessionRegistry.kill)."""

    def __init__(self, name: str):
        super().__init__(f"a sessao '{name}' continua de pe depois do kill-session")
        self.name = name


def _rename_pair_python(old: str, new: str) -> None:
    """No modo Rust o grupo já foi renomeado pela rota antes do tmux e do sidecar (`group.rename`)."""
    from app import groups_bridge
    if not groups_bridge.rust_owns_groups():
        rename_pair(old, new)


class GroupCleanupFailed(ValueError):
    """O nome reusado ainda tem o grupo da sessão antiga e o Rust não o desfez: criar agora faria
    a sessão nova nascer dentro dele."""
    code = "erro_grupo_limpeza_falhou"

    def __init__(self, name: str):
        super().__init__(f"o grupo da sessão antiga '{name}' não foi desfeito; tente de novo")
        self.name = name


def _retire_waiting_runtime(name: str) -> None:
    """Vida antiga do mesmo nome, ainda esperando identidade no runtime, não segura o nome novo."""
    from app import runtime_coordinator
    from app.runtime_adapter import run_sync
    owner = runtime_coordinator.current()
    if owner is not None and owner.loop is not None and owner.managed_queue(name):
        run_sync(lambda: owner.retire_waiting(name), owner.loop)


class SessionRegistry:
    # Cache name -> ultimo jsonl resolvido por sinal CONFIAVEL (cmdline --session-id / fd). De classe
    # (compartilhado entre instancias: api.registry e sse._registry). Estabiliza a resolucao quando o
    # processo que carrega o --session-id SOME transitoriamente (a sessao dirigida por job/harness
    # spawna claude por turno) -> sem isto a resolucao oscilava pro mtime e o watcher do SSE limpava o
    # chat. Atualizado quando um sinal confiavel reaparece (ex: /clear -> session-id novo).
    _jsonl_cache: dict[str, str] = {}
    # DE CLASSE, como os outros caches: api, sse (2) e prune têm instâncias próprias e todas
    # chamam list(); contador por instância fecharia "2 polls" em milissegundos.
    _pair_ausencias: dict[str, float] = {}
    _PAIR_AUSENCIA_MIN_S = 5.0
    # nomes cujo cache veio do fd ABERTO (verdade do FS, nao chute). Mantido entre polls sem fd p/ nao
    # oscilar pro --session-id da cmdline (resume: o id da cmdline nunca vira arquivo). De classe.
    _fd_locked: set[str] = set()
    # DIAG: ultima resolucao logada por nome ("<jsonl>|<tracked>") -> loga so quando MUDA (o momento do
    # split/cross-wire), sem spammar a cada poll. Remover quando o bug de colisao estiver resolvido.
    _last_res: dict[str, str] = {}
    # Statusline por sessao: name -> (monotonic da captura, linha crua ou None). De classe
    # (compartilhado entre api.registry e as instancias do sse) — uma captura serve todos.
    _status_cache: dict[str, tuple[float, Optional[str]]] = {}
    # Contexto e modelo da sessão Claude lidos do transcript:
    # name -> (monotonic da leitura, jsonl, {used, window}, id do modelo).
    # O jsonl vai junto porque /clear troca o arquivo e o valor da conversa anterior não vale mais.
    _context_cache: dict[str, tuple[float, Optional[str], Optional[dict], Optional[str]]] = {}
    # Pid do agente Claude de cada pane, visto na varredura: de onde ler `--model` e a janela.
    _agent_pid: dict[str, int] = {}
    # Texto do spinner ("Hyperspacing… (1m51s · ↓2.1k tokens)") extraido da MESMA captura do sweep:
    # o fast-path de marcador deixa label=None e o card nunca mostrava a barrinha de "trabalhando".
    _label_cache: dict[str, Optional[str]] = {}
    # Banner de limite de uso por sessao TRAVADA: name -> (monotonic, horario de volta ou None).
    # Sessao em limite fica `working` pelo marcador e nunca passaria pela captura; olhar o pane so
    # das travadas, com este cache, e o que liga o radar sem raspar toda sessao a cada poll.
    _limit_cache: dict[str, tuple[float, Optional[str]]] = {}
    _LIMIT_CACHE_S = 30.0
    # name -> (jsonl, mtime, provider, texto, ts). A cauda só é relida quando o transcript muda e a
    # sessão volta a ficar parada; sem isto cada atualização da lista reparsaria todas as conversas.
    _reply_cache: dict[str, tuple[str, Optional[float], str, Optional[str], Optional[float]]] = {}
    # Nomes ja avisados por _agent_pane (Task 5.5): sessao com 2+ panes e nenhum reconhecido como
    # agente. De classe pela MESMA razao das demais acima (list() roda em ambas instancias).
    _SEM_AGENTE_AVISADAS: set[str] = set()

    def __init__(self, projects_dir: Path | None = None):
        self.projects_dir = Path(projects_dir or settings.projects_dir)
        # mtime do transcript cujo pane já foi conferido como parado: não raspa de novo até ele mudar.
        self._idle_conferido: dict[str, float] = {}

    def resolve_jsonl(self, cwd: str, projects_dir: Path | None = None) -> Optional[str]:
        # FALLBACK por cwd: jsonl mais recente do dir do projeto. So usado quando nao ha --session-id
        # nem fd aberto. NAO confiavel com varias sessoes no mesmo cwd (colide) -> por isso o
        # cmdline --session-id (em resolve()) vem primeiro.
        proj = (projects_dir or self.projects_dir) / sanitize_cwd(cwd)
        if not proj.is_dir():
            return None

        def _mtime(f: Path) -> float:
            # arquivo pode sumir entre o glob e o stat (sessao encerrando) -> nao deixar OSError subir
            # ate o /api/sessions virar 500; o sumido vai pro fim da ordenacao (mtime 0).
            try:
                return f.stat().st_mtime
            except OSError:
                return 0.0

        files = sorted(proj.glob("*.jsonl"), key=_mtime, reverse=True)
        return str(files[0]) if files else None

    def _aux_open_jsonls(self, pids: list[int]) -> set[str]:
        # realpaths de jsonl que processos auxiliares (subagente --agent / daemon) seguram abertos AGORA.
        # Excluidos do "mais recente" em _newest_after_clear pra um Task em voo nao virar o transcript da
        # sessao. Best-effort: fd raramente fica aberto em idle -> set vazio na maioria dos polls.
        out: set[str] = set()
        for p in pids:
            cmd = _cmdline(p)
            if not ("daemon" in cmd or "--bg-" in cmd or "--agent" in cmd):
                continue
            # Mesmo filtro do passo 1 do _resolve_tracked_impl, pelo mesmo motivo.
            if _provider_do_argv(cmd.split()) is None:
                continue
            cdir = _config_dir_of(p)
            j = _open_jsonl(p, (cdir / "projects") if cdir else self.projects_dir)
            if j:
                out.add(os.path.realpath(j))
        return out

    def _cwd_has_siblings(self, cwd: str) -> bool:
        # >1 sessao tmux com este MESMO cwd? Com varias, seguir o jsonl mais novo do cwd (newest-by-mtime)
        # cruza o transcript de uma sessao pra outra -> a resolucao por mtime fica ambigua. ponytail: 1
        # fork tmux por chamada; aceitavel (poucas sessoes). Fail-safe: erro -> trata como sem irmaos.
        # Task 5.5 (achado I1 da revisao): `#{pane_current_path}` e o cwd VIVO daquele pane -- um `cd`
        # manual num split muda SO o campo dele. list() agora entrega o cwd do pane do AGENTE; contar
        # so os panes ATIVOS (list_panes_active) deixava as duas pontas olhando cwds diferentes, e uma
        # sessao com split "sem irmao" caia no newest-by-mtime que esta guarda existe pra evitar --
        # exatamente o "sem id" que a Task 5.5 conserta, reaparecendo por outra porta. QUALQUER pane da
        # sessao com esse cwd conta (superset seguro: sobre-contar so empurra pro caminho <sid>.jsonl
        # direto, nunca pro mtime ambiguo).
        # Task 6: a sessao de shell ESCONDIDA nasce com o MESMO cwd do agente (new_hidden_shell usa
        # info.cwd) e SOBREVIVE reatada entre polls -- sem excluir aqui, abrir o shell uma vez faria
        # a sessao contar "irmao" pra sempre, e o resolve_tracked perderia _newest_after_clear (o
        # catch-up do /clear) na sessao pra sempre, mesmo sem NENHUMA outra sessao Claude no cwd.
        try:
            return sum(1 for panes in tmux.list_panes_all().values()
                       if not panes[0].get("hidden") and any(p.get("cwd") == cwd for p in panes)) > 1
        except Exception:
            return False

    def resolve(self, name: str, cwd: str) -> Optional[str]:
        return self.resolve_tracked(name, cwd)[0]

    def _log_change(self, name: str, jsonl: Optional[str], tracked: bool) -> None:
        # DIAG: loga a resolucao SO quando muda pra um nome (baseline no 1o poll, depois so transicoes).
        key = f"{jsonl}|{tracked}"
        if self._last_res.get(name) == key:
            return
        prev = self._last_res.get(name)
        self._last_res[name] = key
        _log.info("RESOLVE name=%s jsonl=%s tracked=%s prev=%s",
                  name, (jsonl or "").rsplit("/", 1)[-1], tracked,
                  (prev or "-").rsplit("/", 1)[-1].split("|")[0])

    def resolve_tracked(self, name: str, cwd: str, pid=_UNSET,
                        children: Optional[dict[int, list[int]]] = None) -> tuple[Optional[str], bool]:
        if rust_owns_list():
            from app import list_bridge
            return list_bridge.resolve(name, cwd, None if pid is _UNSET else pid)
        PYTHON_DISCOVERY["resolve_tracked"] += 1
        jsonl, tracked = self._resolve_tracked_impl(name, cwd, pid, children)
        self._log_change(name, jsonl, tracked)
        return jsonl, tracked

    def _resolve_tracked_impl(self, name: str, cwd: str, pid=_UNSET,
                        children: Optional[dict[int, list[int]]] = None) -> tuple[Optional[str], bool]:
        # Mapeia uma sessao tmux -> o jsonl CERTO + se o vinculo e CONFIAVEL (tracked).
        # tracked=True so com sinal DETERMINISTICO: --session-id do cmdline, fd aberto, ou cache
        # (semeado por um desses / pelo create()). tracked=False = chute newest-by-mtime, que COLIDE
        # com varias sessoes bare no mesmo cwd -> a UI marca "sem id" e desliga o chat (evita mostrar
        # /trocar transcript errado). Determinismo so com --session-id: o "+" do app, ou o wrapper
        # `claude --session-id <uuid>` no terminal.
        # pid/children: quando a listagem ja os tem (pane_pid em lote + mapa /proc unico), evita um fork
        # tmux e uma re-varredura do /proc por sessao. _UNSET = resolve sozinho (caminho single-session).
        if pid is _UNSET:
            pid = tmux.pane_pid(name)
        if pid is not None:
            pids = _descendant_pids(pid, children)
            # jsonls que processos AUXILIARES (subagente --agent / daemon) seguram abertos AGORA: sao
            # transcripts de outra sessao logica -> nunca devem virar o transcript do REPL principal.
            aux_open = self._aux_open_jsonls(pids)
            # 1. fd aberto do REPL = transcript REALMENTE ativo agora (mais preciso que o cmdline, que
            #    congela no boot). Vem ANTES do --session-id: apos um /clear (que rola session-id NOVO
            #    sem mexer no cmdline) o claude passa a escrever num jsonl novo -> o fd aponta pra ele.
            #    Pula os auxiliares (subagente/daemon) p/ nao pegar o transcript de um deles.
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                # So um CLI de agente abre transcript. O resto da arvore (servidores MCP, node, git,
                # shells) nunca casa, e varrer o /proc/<pid>/fd deles e o passo mais caro da
                # listagem: medido nesta maquina, 85% dos readlinks eram de nao-agentes e o
                # resultado era None. `cmd.split()` e nao `_argv` (ao contrario do provider_of_pane,
                # que evita o split por causa do espaco no caminho do node no Windows): daqui pra
                # baixo so roda no Linux, porque fora dele o _open_jsonl ja devolve None sempre.
                if _provider_do_argv(cmd.split()) is None:
                    continue
                cdir = _config_dir_of(p)
                j = _open_jsonl(p, (cdir / "projects") if cdir else self.projects_dir)
                if j:
                    self._jsonl_cache[name] = j
                    self._fd_locked.add(name)  # fd = verdade -> trava p/ os polls sem fd nao reverterem
                    return j, True
            # 1.5. Marcador do hook por cmdline sid: DETERMINISTICO e reescrito a cada evento -> vem
            #      ANTES do fd-lock duravel. Apos um /clear que rola transcript novo escrito em
            #      append-and-close (fd quase nunca aberto no poll) e cujo sid novo NUNCA vai pro
            #      cmdline, o fd-lock ficava preso no transcript PRE-clear e o chat nao migrava. O
            #      marcador sabe o transcript ativo do boot_id -> deixa ele destravar o cache velho.
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                sid = _session_id_from_cmdline(cmd)
                if not sid:
                    continue
                cdir = _config_dir_of(p)
                config_base = cdir if cdir else self.projects_dir.parent
                marker = _active_marker_jsonl(config_base, sid, aux_open)
                if marker:
                    if self._jsonl_cache.get(name) != marker:
                        self._fd_locked.discard(name)  # transcript rolou (/clear|resume) -> solta o lock velho
                    self._jsonl_cache[name] = marker
                    return marker, True
            # fd AUSENTE neste instante: se ja travamos por fd (transcript REAL desta sessao, pego num
            # write anterior), MANTEM o cache. Sem isto, um resume cujo --session-id da cmdline nunca
            # vira arquivo oscilava fd<->id entre writes (e o watcher do SSE resetava o chat).
            if name in self._fd_locked:
                cached = self._jsonl_cache.get(name)
                if cached:
                    return cached, True
                self._fd_locked.discard(name)
            # 2. cmdline --session-id (DETERMINISTICO; app-created sempre, manual com flag). Vale mesmo
            #    sem o arquivo existir ainda (sessao recem-criada) -> o tailer segue quando aparecer.
            #    PULA os processos auxiliares da arvore do claude, que carregam um --session-id PROPRIO
            #    (transitorio) != o do REPL principal -> sem isto resolvia pro jsonl errado/inexistente:
            #      - `claude daemon` + bg-pty-host/spare (sockets em /tmp/cc-daemon-*): contem "daemon"/"--bg-"
            #      - SUB-AGENTES (`--agent`): cada Task/subagent roda seu proprio session-id.
            #    O --session-id CONGELA no boot: o /clear gera um session-id novo e o cmdline segue o
            #    velho -> _newest_after_clear segue o jsonl mais recente do projeto (= transcript pos-clear).
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                sid = _session_id_from_cmdline(cmd)
                if sid:
                    cdir = _config_dir_of(p)
                    proj = (cdir / "projects") if cdir else self.projects_dir
                    # Marcador do hook ja tratado no passo 1.5 (antes do fd-lock). Aqui so o fallback
                    # deterministico por <sid>.jsonl / newest-after-clear quando nao ha marcador.
                    projdir = proj / sanitize_cwd(cwd)
                    sid_jsonl = str(projdir / f"{sid}.jsonl")
                    # _newest_after_clear (segue o jsonl mais NOVO do cwd pra pegar o pos-/clear) so e
                    # seguro com UMA sessao na pasta. Com VARIAS sessoes no mesmo cwd, o jsonl mais novo
                    # de uma (ex: resume/clear) CONTAMINA as outras (vira o transcript delas). Nesse caso
                    # usa o <id>.jsonl DIRETO; o fd (passo 1) ainda corrige /clear+resume da PROPRIA
                    # sessao quando pega o arquivo aberto no write.
                    if self._cwd_has_siblings(cwd):
                        j = sid_jsonl
                    else:
                        j = _newest_after_clear(projdir, sid_jsonl, aux_open)
                    self._jsonl_cache[name] = j
                    return j, True
            # 2.5. Marcador do hook casado por PID (sessao BARE: `claude` sem --session-id, nada no
            #      cmdline). O state_hook grava o pid do REPL no marcador; se ele e descendente deste
            #      pane, o transcript e desta sessao — DETERMINISTICO, vira tracked (o chat liga).
            #      Cobre tambem resume feito por fora. So nao existe marcador antes do 1o evento de
            #      hook da sessao -> cai nos passos seguintes ate o 1o prompt.
            cdir_m = _config_dir_of(pid)
            config_base_m = cdir_m if cdir_m else self.projects_dir.parent
            marker = _marker_by_pids(config_base_m, pids, aux_open)
            if marker:
                self._jsonl_cache[name] = marker
                return marker, True
        # 3. cache: ultimo sinal confiavel. Estabiliza quando o processo com --session-id some
        #    transitoriamente (senao a resolucao oscilava pro mtime e o watcher limpava o chat).
        cached = self._jsonl_cache.get(name)
        if cached:
            return cached, True
        # 4. fallback: mais recente por mtime (ambiguo com varias sessoes bare no mesmo cwd) -> NAO tracked.
        # usa o config dir da sessao (lido do pane pid, herdado pela arvore) pra achar o jsonl certo
        # quando a sessao roda num config dir != o do backend. ponytail: le do pane pid; se um alias
        # setasse CLAUDE_CONFIG_DIR so no exec do claude (nao exportado), cairia no dir do backend.
        cdir = _config_dir_of(pid) if pid is not None else None
        proj = (cdir / "projects") if cdir else self.projects_dir
        return self.resolve_jsonl(cwd, proj), False

    def _seed(self, name: str, jsonl: str, required: bool = False) -> None:
        """Transcript da sessão que acabou de nascer ou trocar de modo: vale antes de o agente
        escrevê-lo. A sessão já existe; se a ponte falhar, a falha já está no diário (`lista.ponte`)
        e o Rust resolve pelo `--session-id` ou pelo sidecar dela. `required`: transferência, em
        que o transcript não se deduz do processo; aí a falha levanta."""
        self._jsonl_cache[name] = jsonl
        if _rust_caches():
            from app import list_bridge
            try:
                list_bridge.seed(name, jsonl)
            except (list_bridge.ListBridgeError, tmux.MuxIndisponivel):
                if required:
                    raise

    def _rename_rust(self, old: str, new: str) -> None:
        """O rename já aconteceu: falha da ponte fica no diário (`lista.ponte`), e o nome velho é
        esquecido de novo antes de qualquer sessão nascer com ele (`_forget` na criação)."""
        if _rust_caches():
            from app import list_bridge
            try:
                list_bridge.rename(old, new)
            except (list_bridge.ListBridgeError, tmux.MuxIndisponivel):
                pass

    def _forget(self, name: str, required: bool = False) -> None:
        """Esquece o nome aqui e no Rust. `required`: criação, antes de qualquer efeito; a falha
        levanta, porque o nome reusado herdaria o cache e abriria a conversa da morta. Nos demais
        a sessão já morreu ou trocou: a falha fica no diário (`lista.ponte`) e a criação seguinte
        do mesmo nome esquece de novo."""
        if _rust_caches():
            from app import list_bridge
            try:
                list_bridge.forget(name)
            except (list_bridge.ListBridgeError, tmux.MuxIndisponivel):
                if required:
                    raise
        self._forget_local(name)

    def _forget_local(self, name: str) -> None:
        self._jsonl_cache.pop(name, None)
        self._fd_locked.discard(name)
        # Nome pode ser reusado por outra sessao: sem isto a nova herdaria a statusline da morta
        # por ate _STATUS_TTL (e o dict cresceria sem poda a cada create/kill).
        self._status_cache.pop(name, None)
        self._context_cache.pop(name, None)
        self._agent_pid.pop(name, None)
        self._label_cache.pop(name, None)
        self._limit_cache.pop(name, None)
        self._reply_cache.pop(name, None)

    def _repl_sid(self, pid, children: Optional[dict[int, list[int]]] = None) -> Optional[str]:
        # --session-id do REPL principal da sessao (pula daemon/agent). Identidade do DONO de um
        # transcript: <sid>.jsonl PERTENCE a sessao cujo cmdline traz esse sid. Usado na guarda de
        # colisao. None se ausente (REPL bare sem flag / sem pid).
        if pid is None:
            return None
        for p in _descendant_pids(pid, children):
            cmd = _cmdline(p)
            if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                continue
            sid = _session_id_from_cmdline(cmd)
            if sid:
                return sid
        return None

    def _dedupe_collisions(self, infos: list[SessionInfo], sids: dict[str, Optional[str]]) -> list[SessionInfo]:
        # 2+ sessoes resolvidas pro MESMO jsonl = colisao (uma tomou emprestado o transcript de outra
        # via marcador/fallback-mtime). So a DONA (cmdline sid == basename do jsonl) mantem; as demais
        # sao rebaixadas (jsonl=None, tracked=False) -> a UI nao duplica e o send nao rota pro terminal
        # errado. Sem dona clara (todas resumiram transcript de terceiro) -> rebaixa todas (nao arriscar
        # transcript errado pra ninguem). Roda no list() (unico ponto com a lista TODA); a resolucao
        # por-sessao segue intacta.
        groups: dict[str, list[SessionInfo]] = {}
        for info in infos:
            if info.jsonl:
                groups.setdefault(os.path.realpath(info.jsonl), []).append(info)
        for jsonl, group in groups.items():
            if len(group) < 2:
                continue
            base = os.path.basename(jsonl).removesuffix(".jsonl")
            owner = next((i for i in group if sids.get(i.name) == base), None)
            if owner is None:
                # Desempate por TRACKED. O teste acima (sid do cmdline == nome do arquivo) so acerta
                # quando o claude escreve no proprio boot_id — e numa sessao RESUMIDA ele nunca faz
                # isso: o cmdline congela no boot_id e o transcript vai pro uuid resumido. E o mesmo
                # fato que o _active_marker_jsonl ja documenta ("o <boot_id>.jsonl do cmdline NUNCA
                # nasce"), so que ali ele e tratado e aqui nao era -> owner=None e o `for` abaixo
                # rebaixava TODAS, inclusive quem tinha vinculo deterministico.
                # MEDIDO nesta maquina: grupo [jeffer1312 (sid=dea09039, jsonl=bdabe8c1, tracked),
                # probepaste (bare, sem sid, mesmo jsonl por chute de mtime)] -> nenhuma dona pelo
                # sid, as duas rebaixadas, e a sessao ATIVA ficava "sem id" na UI (o chat desligava).
                # Alternava entre as sessoes porque o newest-by-mtime que a bare reivindica e sempre
                # o transcript de quem acabou de escrever.
                # tracked=True so vem de sinal DETERMINISTICO (marcador do hook casado por pid da
                # arvore, fd aberto, ou cache semeado por um deles) — prova de propriedade mais forte
                # que o chute newest-by-mtime de uma sessao bare. Exige UNICO: com 2+ tracked no mesmo
                # jsonl nao ha dona obvia (duas resumiram o mesmo transcript), e com 0 tampouco —
                # nos dois casos segue rebaixando todas, como antes.
                tracked = [i for i in group if i.tracked]
                if len(tracked) == 1:
                    owner = tracked[0]
            for info in group:
                if info is owner:
                    continue
                _log.info("COLLISION name=%s dropped borrowed jsonl=%s owner=%s",
                          info.name, base, owner.name if owner else "none")
                info.jsonl = None
                info.tracked = False
        return infos

    @staticmethod
    def _agent_pane(panes: list[dict], children: dict[int, list[int]]) -> dict:
        """Escolhe, entre os panes de UMA sessao, o que roda o agente (Task 5.5).

        list_panes_active() so trazia o pane ATIVO — e "ativo" e por JANELA, nao por sessao: uma
        segunda janela/split (o botao `+` da Task 6) fica marcada ativa TAMBEM, e o antigo dedup por
        nome ficava com a PRIMEIRA da varredura, arbitrario. Com o agente numa janela e o shell na
        outra em primeiro plano, provider/jsonl/pane_id saiam todos do pane ERRADO (medido: o shell
        vira "sem id" na lista).
        Reusa o predicado ESTRITO do agentpane (_pane_do_agente, Task 1) e o MESMO mapa /proc que
        list() ja construiu pra sessao inteira -> zero fork NOVO (achado menor da revisao: a leitura
        de /proc nao e zero, e sim proporcional ao numero de panes candidatos da sessao — barata
        porque o mapa `children` ja esta pronto, mas nao e de graca). Nenhum pane bate -> cai no
        pane ATIVO, o comportamento de sempre (None = nao sei, nao decide um comportamento novo
        sozinho).
        """
        if len(panes) > 1:
            # Achado menor da revisao: com 2+ panes do agente na MESMA sessao (caso raro), o ATIVO
            # ganha o desempate -- preserva o comportamento de antes desta task pra esse caso, em vez
            # de arbitrario "o primeiro da varredura".
            for p in sorted(panes, key=lambda p: not p["active"]):
                if p["pid"] is not None and agentpane._pane_do_agente(p["pid"], children):
                    return p
            name = panes[0]["name"]
            if name not in SessionRegistry._SEM_AGENTE_AVISADAS:
                # Falha aparece, nao some — mas UMA vez por nome (list() e polled a cada segundo;
                # logar em TODO poll enquanto a sessao seguir sem agente reconhecido enche o journal
                # a toa). ponytail: dedup por NOME nunca expira (nem no kill/recria, ao contrario do
                # agentpane._AVISADAS) — pior caso e uma sessao rara, apos recriada, ficar calada de
                # novo neste caso; upgrade so se virar reclamacao real.
                SessionRegistry._SEM_AGENTE_AVISADAS.add(name)
                _log.warning("list: %r tem %d panes e nenhum parece do agente; "
                             "caindo no pane ATIVO", name, len(panes))
        return next((p for p in panes if p["active"]), panes[0])

    def list(self, newer_than: float | None = None) -> list[SessionInfo]:
        # `newer_than` (época): sessão criada há menos de 1 s, só vale descoberta começada depois.
        # No Python quem garante isso é o `_guardar_snap(forcar=True)`, que relê os processos.
        if rust_owns_list():
            from app import list_bridge
            # A descoberta do Rust não traz as `orq` (na lista elas vêm dos fatos); sem elas, quem
            # procura a sessão pelo nome (painel, histórico) não a acha.
            return list_bridge.discover(newer_than) + _orq_infos()
        PYTHON_DISCOVERY["list"] += 1
        # Resolucao de jsonl/tracked de todas as sessoes. Otimizado: UM mapa /proc + UMA chamada tmux
        # (pane_pid em lote) reusados por sessao -> O(P + S·descendentes) em vez de O(S·P). NAO calcula
        # state (sai 'idle' default): este caminho so resolve transcript; quem quer state usa
        # list_with_state(). Usado por varios endpoints que so precisam do jsonl por nome.
        children = _proc_children_map()
        from app.share_life import session_life
        out = []
        sids: dict[str, Optional[str]] = {}
        pane_groups = tmux.list_panes_all()
        terminal_births = {name: panes[0].get("session_created") for name, panes in pane_groups.items() if panes}
        for panes in pane_groups.values():
            # Sessao de shell ESCONDIDA (Task 6, botao "+" do painel de terminal): marcada por
            # opcao de usuario tmux (@cp_hidden), herdada por TODOS os panes/janelas da sessao (
            # confirmado na revisao), lida de carona no MESMO list-panes acima -- sem isto ela
            # viraria CARD nas tres views (lista, board, canvas), porque pane nao reconhecido vira
            # Claude por padrao logo abaixo. list_with_state() reusa esta mesma lista (nao chama
            # list_panes_all de novo), entao o pulo vale nas duas.
            #
            # Checado ANTES do `_agent_pane` (achado da revisao, minor): usuario dividindo o
            # proprio shell escondido (2+ panes, nenhum "agente") faria `_agent_pane` nao achar
            # ninguem, logar o warning "nenhum parece do agente" pra sempre (suja
            # `_SEM_AGENTE_AVISADAS`, que nunca expira) e pagar a descida de /proc por pane -- tudo
            # sobre uma sessao que o app ignora DE PROPOSITO. Qualquer pane serve pra checar: a
            # marca e por sessao, todos concordam.
            if panes[0].get("hidden"):
                _log.debug("list: sessao %r pulada (marcada @cp_hidden)", panes[0]["name"])
                continue
            p = self._agent_pane(panes, children)
            # A TUI Codex agora vive no tmux, mas sua identidade/historico continuam vindo do
            # sidecar + rollout. Nao a tratar tambem como Claude (duplicaria a sessao e tentaria
            # resolver ~/.claude/projects).
            if codex_sessions.exists(p["name"]):
                # O filtro e por NOME. Se um sidecar ficar ORFAO (crash antes do cleanup) e alguem
                # criar uma sessao Claude com o mesmo nome, ela sumiria da lista SEM explicacao --
                # o usuario perderia acesso a uma sessao viva. Nao da pra distinguir aqui sem custo,
                # entao pelo menos NAO e silencioso: o log diz qual nome foi filtrado e por que.
                _log.debug("list: pane %r filtrado por sidecar Codex de mesmo nome", p["name"])
                continue
            # Pi anda no MESMO caminho tmux que o Claude (pane real, mtime real) — so a resolucao do
            # jsonl muda: o --session-id nao sobrevive no cmdline (Task 0, fato 7) e resolve_tracked
            # cairia no fallback newest-by-mtime, que pegaria o transcript do CLAUDE do mesmo cwd (a
            # regressao mais cara desta task). Resolve pelo bilhete da extensao / env do wrapper.
            prov, pid_agente = agente_do_pane(p["pid"], children)
            # Durante o boot só há shell: a escolha da criação já identifica o dono.
            if pid_agente is None and p.get("provider"):
                prov = p["provider"]
            # Quem declara conta e motor e o processo do agente, nao o pane: numa sessao aberta a mao
            # o pane e o shell, e o shell nao tem CLAUDE_CONFIG_DIR nem CP_ENGINE.
            pid_env = pid_agente or p["pid"]
            if prov in ("pi", "omp"):
                jsonl = pi_session_file(p.get("pane_id", ""), p["pid"], p["cwd"], prov)
                # tracked segue o TRANSCRITO, nao o provider. O bilhete/env sao deterministicos
                # (nunca um chute como o newest-by-mtime do Claude), mas quando NENHUM dos dois
                # resolve um arquivo nao ha vinculo nenhum: /events e /history exigem info.jsonl e
                # devolvem 404. Com o True fixo a lista mostrava um card clicavel que abria um chat
                # quebrado e, por ser "tracked", sem nenhuma das afordancias de sessao sem id. Com
                # False as duas views cinzam a linha e explicam. E temporario por construcao: no 1o
                # turno o Pi escreve o transcript, o bilhete passa a resolver e a proxima varredura
                # devolve tracked=True sozinha (list() e polled).
                tracked = jsonl is not None
            elif prov == "kimi":
                # Mesmo contrato do Pi acima: so o bilhete do hook (kimi_session_file) liga o pane
                # ao wire — sem ele NAO ha fallback (o Kimi nao aceita id escolhido pelo caller).
                # Temporario: a sessao Kimi so nasce no 1o prompt ("No session yet" da TUI), entao
                # toda sessao recem-criada fica untracked ate o 1o turno.
                jsonl = kimi_session_file(p.get("pane_id", ""), p["pid"], p["cwd"])
                tracked = jsonl is not None
            elif prov == "codex":
                # Chegar aqui significa pane de Codex SEM sidecar (o filtro acima ja tirou os que
                # tem): a TUI subiu e ainda nao abriu a thread. Nao ha transcript a resolver — o
                # rollout so nasce com a thread —, e cair no resolve_tracked pescaria o jsonl do
                # Claude do mesmo cwd. Some sozinho: o lancador grava o sidecar e a proxima
                # varredura ja acha a sessao pelo caminho normal.
                jsonl, tracked = None, False
            else:
                jsonl, tracked = self.resolve_tracked(p["name"], p["cwd"], p["pid"], children)
            link = ThenLink(p["name"]).get()
            pair = PairLink(p["name"]).get()
            # Transcript de chute (untracked) pode ser de outra sessão: não decide onde esta está.
            loc = worktrees.locate(prov, p["cwd"], jsonl if tracked else None)
            info = SessionInfo(name=p["name"], cwd=p["cwd"], jsonl=jsonl, tracked=tracked,
                               lifecycle_id=session_life(p["name"], meta=None, birth=p.get("session_created")),
                               branch=loc.branch, worktree=loc.worktree,
                               worktree_path=loc.worktree_path, worktree_gone=loc.worktree_gone, git_cwd=loc.git_cwd,
                               then_target=link.get("target") if link else None,
                               pair_peers=pair.get("peers") if pair else None,
                               pair_external=_pair_external(p["name"], pair.get("peers") if pair else None),
                               pair_gid=pair.get("gid") if pair else None,
                               pair_task=pair.get("task") if pair else None)
            if prov in ("pi", "omp", "kimi", "codex"):
                info.provider = prov
            # Motor da sessão, do mesmo pid que já resolve o config_dir. É uma leitura de
            # /proc/<pid>/environ por sessão (a mesma ordem de custo do _config_dir_of ao lado) —
            # não é de graça, mas é local e sem rede. Feature em tick do SSE tem que ser barata.
            info.engine = _engine_of(pid_env) if pid_env else None
            info.engine_account = procinfo._env_var_of(pid_env, "CP_ENGINE_ACCOUNT") if pid_env and info.engine else None
            if prov == "claude" and pid_env:
                self._agent_pid[p["name"]] = pid_env
            # Conta pra pílula de cota (id do /api/cotas): com motor, a chave do engines.json; sem
            # motor e Claude, o config dir do pane — ou o default (~/.claude) quando o processo não
            # declara CLAUDE_CONFIG_DIR (o fallback é idiom dos call sites, não do _config_dir_of).
            # O .resolve() casa com o id do /api/cotas, que sai de list_config_dirs() RESOLVIDO —
            # com $HOME symlinkado ou path não-canônico de alias, o id cru nunca casaria e a pílula
            # degradava calada pro pior-geral.
            if info.engine:
                info.conta = (procinfo._env_var_of(pid_env, "CP_ENGINE_CREDENTIAL_ID")
                              if info.engine_account else f"chave:{info.engine}")
            elif prov == "kimi":
                # Sessão Kimi sem motor gasta o provider do default_model do config dela
                # ("apikey/k3" -> conta "kimi:apikey"). Cache por mtime dentro de cotas — isto
                # roda por sessão a cada varredura.
                from app import cotas
                padrao = cotas.provider_padrao_kimi()
                info.conta = f"kimi:{padrao}" if padrao else None
            elif prov in ("pi", "omp"):
                # Sessão Pi/omp gasta a credencial do modelo escolhido NELA (o `current.provider` do
                # sidecar do catálogo), que pode ser a mesma chave Kimi/motor que já é uma conta
                # desta lista. Provider sem chave conhecida (OAuth do Codex, provedor só do Pi)
                # segue None, e a pílula cai no pior-geral como antes.
                from app import cotas, pi_models
                cfg_pi = _config_dir_of(pid_env) if pid_env else None
                atual = pi_models.provider_atual(jsonl, cfg_pi) if jsonl else None
                info.conta = cotas.conta_de_provider_pi(atual)
            elif prov == "codex":
                # A origem é a raiz do Codex, mesmo sem `auth.json`: a identidade da sessão não pode
                # desaparecer só porque a conta está desconectada.
                codex_home = str(codex_contas.default_home().resolve(strict=False))
                info.codex_home = codex_home
                info.conta = f"codex:{codex_home}"
            else:
                cdir = (_config_dir_of(pid_env) if pid_env else None) or (Path.home() / ".claude")
                info.conta = f"claude:{Path(cdir).resolve()}"
            out.append(info)
            sids[p["name"]] = self._repl_sid(p["pid"], children)
        # Guarda de colisao: 2+ sessoes no mesmo jsonl -> so a dona mantem (mata a duplicata/cross-wire).
        self._dedupe_collisions(out, sids)
        # Sessoes Codex: a TUI vive no tmux, mas a identidade vem dos sidecars duraveis (sobrevivem
        # a restart; o historico esta no rollout). O client vivo e reaberto sob demanda.
        for meta in codex_sessions.list_all(include_incomplete=True):
            codex_home = str(Path(meta.get("codex_home") or codex_contas.default_home())
                             .expanduser().resolve(strict=False))
            cwd = cwd_atual(meta)
            loc = worktrees.locate("codex", cwd, meta.get("rollout_path"))
            out.append(SessionInfo(
                name=meta["name"], cwd=cwd, jsonl=meta.get("rollout_path") or None,
                lifecycle_id=session_life(meta["name"], meta=meta, birth=terminal_births.get(meta["name"])),
                provider="codex", tracked=True, conta=f"codex:{codex_home}",
                codex_home=codex_home, headless=bool(meta.get("headless")),
                codex_service_tier=meta.get("service_tier"),
                branch=loc.branch, worktree=loc.worktree,
                worktree_path=loc.worktree_path, worktree_gone=loc.worktree_gone, git_cwd=loc.git_cwd,
                then_target=(ThenLink(meta["name"]).get() or {}).get("target"),
                pair_peers=(PairLink(meta["name"]).get() or {}).get("peers"),
                pair_external=_pair_external(meta["name"], (PairLink(meta["name"]).get() or {}).get("peers")),
                pair_gid=(PairLink(meta["name"]).get() or {}).get("gid"),
                pair_task=(PairLink(meta["name"]).get() or {}).get("task"),
            ))
        # Sessões Claude SEM terminal: não há pane; a identidade vem do sidecar e o transcript
        # é o .jsonl comum do Claude (sid gravado no sidecar, atualizado pelo adapter no /clear).
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        hl = get_adapter(CLAUDE_HEADLESS)
        for meta in headless_sessions.list_all():
            cwd = cwd_atual(meta)
            jsonl = hl.transcript_path_de(meta)
            loc = worktrees.locate("claude", cwd, jsonl)
            cdir = meta.get("config_dir")
            out.append(SessionInfo(
                name=meta["name"], cwd=cwd, jsonl=jsonl,
                lifecycle_id=session_life(meta["name"], meta=meta, birth=None),
                provider="claude", headless=True, tracked=True, engine=meta.get("engine"),
                engine_account=meta.get("engine_account"),
                conta=(meta.get("engine_credential_id") if meta.get("engine_account") else
                       f"chave:{meta['engine']}" if meta.get("engine") else
                       f"claude:{Path(cdir or Path.home() / '.claude').resolve()}"),
                branch=loc.branch, worktree=loc.worktree,
                worktree_path=loc.worktree_path, worktree_gone=loc.worktree_gone, git_cwd=loc.git_cwd,
                then_target=(ThenLink(meta["name"]).get() or {}).get("target"),
                pair_peers=(PairLink(meta["name"]).get() or {}).get("peers"),
                pair_external=_pair_external(meta["name"], (PairLink(meta["name"]).get() or {}).get("peers")),
                pair_gid=(PairLink(meta["name"]).get() or {}).get("gid"),
                pair_task=(PairLink(meta["name"]).get() or {}).get("task"),
            ))
        out.extend(_orq_infos())
        _decorate_transfers(out)
        return out

    async def _radar_de_limite(self, infos: list[SessionInfo], raspadas: set[str]) -> None:
        """Preenche limited/limit_reset das sessoes TRAVADAS que o fast-path de marcador nao raspou.
        Uma sessao esperando o limite voltar e `working` pelo hook e sem transcript avancando —
        exatamente `stalled` —, e so ela paga a captura, uma vez a cada _LIMIT_CACHE_S."""
        # Codex nunca raspa o pane (a TUI dele nao tem o rodape do Claude Code) — fica de fora.
        alvos = [i for i in infos if getattr(i, "stalled", False) and i.name not in raspadas
                 and getattr(i, "provider", "claude") != "codex" and not getattr(i, "headless", False)]
        agora = time.monotonic()
        frescos = [i for i in alvos
                   if agora - self._limit_cache.get(i.name, (0.0, None))[0] > self._LIMIT_CACHE_S]
        if frescos:
            frames = await asyncio.gather(
                *[asyncio.to_thread(tmux.capture_pane, i.name) for i in frescos], return_exceptions=True)
            for i, f in zip(frescos, frames):
                if isinstance(f, str):
                    reset = rate_limit_reset(f)
                else:
                    # tmux engasgado: preserva o ultimo valor bom (mesma regra do sweep de statusline).
                    _log.debug("radar de limite: captura falhou pra %s: %r", i.name, f)
                    reset = self._limit_cache.get(i.name, (0.0, None))[1]
                self._limit_cache[i.name] = (agora, reset)
        for i in alvos:
            i.limit_reset = self._limit_cache.get(i.name, (0.0, None))[1]
            i.limited = i.limit_reset is not None

    async def list_with_state(self, infos: Optional[list[SessionInfo]] = None,
                              state_only: bool = False) -> list[SessionInfo]:
        # Listagem COM estado vivo por sessao (pro /api/sessions). Faz a resolucao otimizada (sync, num
        # thread) e por cima classifica o pane de cada sessao concorrentemente. `infos` opcional: um
        # snapshot ja resolvido (ex: cache compartilhado dos pollers do SSE) pula a re-resolucao.
        # `state_only`: só o estado (fatos da lista, `list_facts`); a decoração é do Rust.
        if not state_only:
            if await rust_owns_list_async():
                from app import list_bridge
                rows = await asyncio.to_thread(list_bridge.snapshot)
                if infos is not None:
                    wanted = {i.name for i in infos}
                    rows = [r for r in rows if r.name in wanted]
                return rows
            PYTHON_DISCOVERY["list_with_state"] += 1
        if infos is None:
            infos = await asyncio.to_thread(self.list)
        # Orquestrador: sem pane, hook, statusline nem git próprio. O estado sai só da atividade da
        # linha do tempo, e a linha fica fora de tudo abaixo (captura de pane, marcador, radar).
        from app.conversation_transfer import TransferPhase
        terminal_phases = {p.value for p in (TransferPhase.COMPLETE, TransferPhase.REJECTED, TransferPhase.ROLLED_BACK)}
        transfers = [i for i in infos if i.transfer_phase and i.transfer_phase not in terminal_phases]
        infos = [i for i in infos if not i.transfer_phase or i.transfer_phase in terminal_phases]
        orqs = [i for i in infos if getattr(i, "provider", "claude") == "orq"]
        if orqs:
            def _orq_state():
                for i in orqs:
                    i.state, i.last_activity = orq_runs.activity(i.jsonl)
            await asyncio.to_thread(_orq_state)
            infos = [i for i in infos if getattr(i, "provider", "claude") != "orq"]
        async def decorate_access(rows):
            active = share_store.active_sessions()
            for row in rows:
                row.shared = row.name in active
            if guest_users.has_claims():
                def owners():
                    for row in rows:
                        row.owner = guest_users.owner_name(row.name)
                await asyncio.to_thread(owners)

        if not infos:
            await decorate_access(orqs + transfers)
            return orqs + transfers
        # Estado pela marca dos hooks quando existe (custo ~0); senao cai no pane (fallback).
        # NOTA: o sweep de STATUSLINE (mais abaixo) captura pane mesmo de sessao com marcador —
        # a statusline nao tem outra fonte. O "custo ~0" continua valendo pra CLASSIFICACAO; o
        # sweep e limitado a _STATUS_BUDGET capturas por chamada com TTL de _STATUS_TTL.
        def _sid(jsonl):
            # session_key, NAO Path().stem: no Kimi todo transcript se chama wire.jsonl, entao o stem
            # e "wire" pra TODA sessao — get_state("wire") nunca casava o marcador (que o hook grava
            # sob o session_id) e toda sessao Kimi caia no fallback de pane. La o spinner e fase de
            # lua, fora de SPINNER_GLYPHS, entao sessao trabalhando aparecia OCIOSA na lista.
            return session_key(jsonl) if jsonl else None
        # `corrige_ocioso_kimi` LE o fim do wire.jsonl (state._kimi_turno_aberto) quando o mtime
        # contradiz um marcador ocioso — I/O de arquivo, nao mais um stat. Esta corrotina e awaitada
        # DIRETO no event loop (/api/sessions, o poller do SSE, o stall_watch), entao a leitura vai
        # pro threadpool numa tacada so: mesma regra do git status em `_decorate_git`, e o mesmo
        # incidente que ela documenta — feature lenta na corrotina congela o backend pra TODAS as
        # sessoes e conexoes, nao so pra dona do arquivo grande.
        kimis = [i for i in infos if getattr(i, "provider", "claude") == "kimi"]
        corrigidos: dict[str, object] = {}
        # Aprovacao pendente do Kimi (plano/comando/arquivo): sai do WIRE, nao do pane — ver
        # state.aprovacao_kimi. Aqui isso e o que faz o card mostrar "aguardando" com os botoes: no
        # painel aberto o turno segue ABERTO no wire, entao `corrige_ocioso_kimi` promove a sessao
        # pra "working" e ela some da coluna de quem espera resposta. Vai na MESMA thread da
        # correcao — as duas leem o rabo do mesmo arquivo.
        aprovacoes: dict[str, tuple[str, list[str]]] = {}
        if kimis:
            brutos = {i.name: hook_state.get_state(_sid(i.jsonl)) for i in kimis}

            def _kimi_sweep():
                corr, aprov = {}, {}
                for i in kimis:
                    corr[i.name] = _kimi_corrige_ocioso(i, brutos[i.name])
                    # O pane responde a outra metade da pergunta ("o painel esta na tela AGORA?"),
                    # sem a qual uma sessao RETOMADA sobre um wire com pedido orfao nasceria com
                    # botoes de um painel inexistente (ver state.aprovacao_kimi). O callable so e
                    # chamado DEPOIS de o wire dizer que ha pedido pendente, que e raro — este
                    # caminho nao acrescenta captura nenhuma ao poll normal. (O sweep de statusline,
                    # mais abaixo, captura por conta propria; e outro orcamento, com TTL.)
                    # Pane ilegivel -> sem prova de tela, sem botao.
                    try:
                        a = aprovacao_kimi(i.jsonl, lambda nome=i.name: tmux.capture_pane(nome))
                    except Exception:
                        a = None
                        _log.debug("capture_pane falhou lendo aprovacao de %s", i.name,
                                   exc_info=True)
                    if a is not None:
                        aprov[i.name] = a
                return corr, aprov

            corrigidos, aprovacoes = await asyncio.to_thread(_kimi_sweep)
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        hl = get_adapter(CLAUDE_HEADLESS)
        pending = []  # infos sem marcador (ou awaiting) -> precisa raspar o pane
        pendente_sem_thread = []  # Codex antes da thread -> raspa o pane SO pra achar menu
        for info in infos:
            # Codex compartilha o estado nativo com o chat; o hook cobre conexões indisponíveis.
            # Nunca classifica o pane: a TUI não tem a régua/composer usados pelo Claude.
            if getattr(info, "provider", "claude") == "codex":
                info.last_activity = _jsonl_mtime(info.jsonl)
                if not info.jsonl:
                    # Sem thread, nenhum cache de uma sessão anterior pertence a esta abertura. Só
                    # o daqui: isto roda nos fatos que o Rust pediu, dentro do laço.
                    self._forget_local(info.name)
                    # Janela entre o pane nascer e o lancador gravar o sidecar: nao ha rollout, e
                    # tanto a chave do marcador quanto a leitura do turno EXIGEM um caminho
                    # (session_key(None) levanta TypeError). Sem esta saida, uma sessao Codex
                    # recem-criada derrubaria a lista INTEIRA — todas as sessoes de todo mundo.
                    #
                    # Mas e justamente aqui que a TUI costuma estar PERGUNTANDO alguma coisa
                    # (aprovar os hooks que a integracao escreveu, escolher o login), e sem isto a
                    # unica saida era um `tmux attach` na maquina. O pane so e raspado por MENU: a
                    # ressalva acima (as duas ultimas linhas virariam uma segunda statusline) vale
                    # pro Codex JA rodando, nao pra um seletor numerado, que e o que `classify`
                    # reconhece. Sem menu, o lançador pode informar a etapa da preparação.
                    if not info.headless:
                        pendente_sem_thread.append(info)
                    continue
                codex = get_adapter("codex")
                snapshot = codex.snapshot(info.name, _sid(info.jsonl))
                marker = hook_state.get_state(_sid(info.jsonl))
                if snapshot is not None:
                    info.state, info.label = snapshot.state, snapshot.label
                    info.codex_service_tier = snapshot.codex_service_tier
                elif marker and marker[0] != "awaiting_input":
                    # awaiting_input nao existe no Codex (o evento equivalente nao existe la); se
                    # aparecer, e marcador de outra coisa e nao vale mais que o default.
                    info.state = marker[0]
                elif await asyncio.to_thread(codex_turno_aberto, info.jsonl):
                    # Turno andando no rollout e marcador nenhum = o hook nao esta rodando, e a
                    # unica causa conhecida e hook nao aprovado na TUI do Codex. Dizer isso e o que
                    # torna visivel o unico modo de falha deste desenho — calado, a sessao ficaria
                    # eternamente "ociosa" enquanto trabalha.
                    info.problema = "codex_hooks_nao_aprovados"
                if info.state != "working":
                    self._label_cache.pop(info.name, None)
                info.pending_questions, info.question = codex.async_question_status(info.name)
                if info.pending_questions and info.state == "idle":
                    info.state = "awaiting_input"
                if info.headless:
                    pergunta, opcoes = codex.aprovacao_pendente(info.name)
                    if pergunta:
                        info.state, info.question, info.options = "awaiting_input", pergunta, opcoes
                    info.problema = codex.problema_de(info.name) or info.problema
                continue
            if getattr(info, "headless", False):
                # Claude sem terminal: NUNCA raspa pane (não há). Processo vivo responde pelo
                # estado (permissão/pergunta pendente inclusive); parado, vale o marcador do hook,
                # e sem marcador é ocioso.
                info.last_activity = _jsonl_mtime(info.jsonl)
                snap = hl.snapshot(info.name)
                if snap is not None:
                    info.state, info.label = snap.state, snap.label
                    info.question, info.options = snap.question, snap.options
                    info.status_line = snap.status_line
                    if snap.codex_question:
                        info.state = "awaiting_input"
                        info.question = snap.codex_question["questions"][0]["question"]
                else:
                    marker = hook_state.get_state(_sid(info.jsonl))
                    info.state = marker[0] if marker and marker[0] in ("working", "idle") else "idle"
                prob = hl.problema_de(info.name)
                if prob:
                    info.problema = prob[0]
                continue
            if getattr(info, "provider", None) == "claude" and not getattr(info, "problema", None):
                from app.runtime_adapter import runtime_problem
                if problem := runtime_problem(info.name):
                    info.problema = problem[0]
            aprov = aprovacoes.get(info.name)
            if aprov is not None:
                # Wire manda: o painel de aprovacao esta na tela AGORA. Nao entra no `pending` (nao
                # ha o que raspar — os rotulos nao estao no pane em formato que este modulo leia) e
                # nao mexe no hook_state: o marcador volta a valer sozinho quando a aprovacao for
                # resolvida, sem promover/rebaixar nada.
                info.state = "awaiting_input"
                info.question, info.options = aprov
                info.last_activity = _jsonl_mtime(info.jsonl)
                continue
            marker = (corrigidos[info.name] if info.name in corrigidos
                      else hook_state.get_state(_sid(info.jsonl)))
            # Marker autoritativo pra working/idle/dead (custo ~0). Pra awaiting_input o marcador NAO
            # carrega a pergunta -> raspa o pane (junto das sem-marcador) pra pegar question/options.
            # LIMITACAO CONHECIDA (rate-limit radar, feature #8): este fast-path PULA a captura do pane,
            # entao rate_limit_reset() NUNCA roda por aqui -> limited/limit_reset ficam no default
            # (False/None). Uma sessao rate-limited fica working/idle (o banner nao e menu), logo cai
            # SEMPRE neste caminho de marcador -> na pratica o chip "limitado"/notify_limited/auto-resume
            # so disparam pela sessao com o chat aberto (StateMonitor raspa o pane), nunca pelo radar da
            # lista. NAO corrigido de proposito: fazer o watchdog raspar o pane de toda sessao working/idle
            # a cada poll so faz sentido depois que _LIMIT_RE (app/state.py) for calibrado contra o banner
            # REAL — hoje e um chute nao-calibrado, entao a deteccao nao funcionaria de verdade mesmo com
            # a plumbing pronta. Calibrar _LIMIT_RE primeiro; so entao vale mover a deteccao pro watchdog.
            # awaiting_input SEMPRE raspa o pane (o marcador nao carrega question/options). O que
            # segurava a tempestade de capture_pane era marcador awaiting PRESO — a Notification de
            # "idle 60s" do Claude Code chega DEPOIS do Stop e nada corrigia, entao a sessao parada
            # raspava a cada poll (e, com o fast-path stale antigo, mostrava "aguardando" falso pra
            # sempre). Corrigido na RAIZ: pane raspado sem menu REBAIXA o marcador pra idle
            # (demote_awaiting, abaixo) -> proximo poll cai no fast-path de marcador como idle.
            mtime = _jsonl_mtime(info.jsonl) if marker else None
            if (getattr(info, "provider", "claude") == "claude" and marker and marker[0] == "idle" and mtime is not None
                    and mtime > marker[1] + _IDLE_STALE_S and self._idle_conferido.get(info.name) != mtime):
                # Transcript escrito depois do idle (fora a folga do resumo pós-Stop) é turno aberto sem
                # UserPromptSubmit, como a volta de um agente em segundo plano. Decide o pane com o spinner animando.
                pending.append(info)
            elif marker and marker[0] != "awaiting_input":
                info.state = marker[0]
                info.last_activity = mtime
                if marker[0] != "working":
                    # Turno acabou (hook e autoritativo): o spinner cacheado e do PASSADO — sem
                    # isto o proximo working herdava a barrinha do turno anterior como se fosse
                    # ao vivo (label fantasma).
                    self._label_cache.pop(info.name, None)
            else:
                pending.append(info)
        if pending:
            frames = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, info.name) for info in pending])
            classified = [classify(t) for t in frames]
            spinners = [_live_spinner(t) for t in frames]
            spin_idx = [k for k, c in enumerate(classified) if c[0] == "working"]
            if spin_idx:
                await asyncio.sleep(0.15)
                f2 = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, pending[k].name) for k in spin_idx])
                for j, k in enumerate(spin_idx):
                    sp2 = _live_spinner(f2[j])
                    if sp2 is None or sp2 == spinners[k]:
                        classified[k] = ("idle", None, None, None)
            for info, c, frame in zip(pending, classified, frames):
                info.state = c[0]
                info.label = c[1]
                info.question = c[2]
                info.options = c[3]
                info.last_activity = _jsonl_mtime(info.jsonl)
                if c[0] == "idle" and info.last_activity is not None:
                    self._idle_conferido[info.name] = info.last_activity
                else:
                    self._idle_conferido.pop(info.name, None)
                # Pane (verdade) contradisse marcador awaiting (Notification de idle-60s, nao menu):
                # rebaixa pra idle no hook_state (mapa+sidecar) — mata o "aguardando" fantasma e
                # devolve a sessao ao fast-path (anti-tempestade). Grace: ver _AWAITING_DEMOTE_GRACE_S.
                sid = _sid(info.jsonl)
                m = hook_state.get_state(sid)
                if (m and m[0] == "awaiting_input" and c[0] != "awaiting_input"
                        and time.time() - m[1] > _AWAITING_DEMOTE_GRACE_S):
                    hook_state.demote_awaiting(sid)
                # Rate-limit radar (feature #8): so pane-derivado, entao so nas infos raspadas aqui
                # (marker path fica com o default False/None, igual a label/question/options).
                info.limit_reset = rate_limit_reset(frame)
                info.limited = info.limit_reset is not None
                self._limit_cache[info.name] = (time.monotonic(), info.limit_reset)
                # Statusline + label de graca: o frame ja foi capturado pra classificar.
                self._status_cache[info.name] = (time.monotonic(), _pane_status(frame))
                self._label_cache[info.name] = c[1]
        if pendente_sem_thread:
            quadros = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, i.name)
                                            for i in pendente_sem_thread])
            for info, frame in zip(pendente_sem_thread, quadros):
                info.startup_steps = [linha.removeprefix("hangar-codex-tui: ")
                                      for linha in frame.splitlines()
                                      if linha.startswith("hangar-codex-tui: ")]
                menu = menu_codex(frame)
                if menu:
                    info.state, (info.question, info.options) = "awaiting_input", menu
                elif any(line.strip() == "Pressione Enter para fechar esta sessão." for line in frame.splitlines()):
                    info.state = "awaiting_input"
                    info.problema = "codex_abertura_falhou"
                    info.label = info.startup_steps[-1] if info.startup_steps else None
                elif info.startup_steps:
                    info.state = "working"
                    info.label = info.startup_steps[-1]
        # Pergunta que o pane nao mostra (o menu rolou pra fora — ver askquestion.pergunta_aberta).
        # FORA dos dois ramos acima de proposito: com marcador de hook a sessao nem raspa o pane, e
        # era justamente ali que a pergunta sumia. So pras que ficaram SEM menu — com menu visivel
        # quem manda e o pane. Em thread e em lote, como as capturas: le disco, e isto e awaitado
        # direto no event loop (mesma regra do git status em _decorate_git).
        sem_menu = [i for i in infos
                    if i.state != "awaiting_input" and not getattr(i, "options", None) and i.jsonl
                    and not getattr(i, "headless", False)]   # pergunta dessa vem do processo, não do sidecar
        if sem_menu:
            # return_exceptions: sidecar e conveniencia e nao pode derrubar a lista INTEIRA de
            # sessoes — mesma regra do git status em _decorate_git (incidente de 2026-07-23).
            pends = await asyncio.gather(
                *[asyncio.to_thread(pergunta_aberta, _sid(i.jsonl)) for i in sem_menu],
                return_exceptions=True)
            for info, q in zip(sem_menu, pends):
                if isinstance(q, Exception):   # nao BaseException: CancelledError nao vira warning
                    _log.warning("askq: leitura da pergunta pendente falhou sessao=%s",
                                 info.name, exc_info=q)
                    continue
                if q is None:
                    continue
                info.state = "awaiting_input"
                info.label = None
                info.question = q.questions[0].question
                info.options = [o.label for o in q.questions[0].options]
        # Statusline pros cards (modelo/contexto/⚡5h/📅7d): cache com TTL — capturar o pane de TODAS
        # por tick seria a tempestade de forks que o fast-path de marcador evita. No maximo
        # _STATUS_BUDGET capturas por chamada, das entradas mais VELHAS do cache; quem foi raspada
        # acima ja atualizou de graca. ponytail: cadencia ~(N/_STATUS_BUDGET)×poll — com 15 sessoes
        # e poll 1.5s, ciclo completo ~11s; statusline muda devagar, atraso e invisivel.
        now_m = time.monotonic()
        stale = [
            i for i in infos
            if getattr(i, "provider", "claude") != "codex" and not getattr(i, "headless", False)
            and all(i is not p for p in pending)
            and now_m - self._status_cache.get(i.name, (0.0, None))[0] > _STATUS_TTL
        ]
        stale.sort(key=lambda i: self._status_cache.get(i.name, (0.0, None))[0])
        for info in stale[:_STATUS_BUDGET]:
            try:
                pane = await asyncio.to_thread(tmux.capture_pane, info.name)
                self._status_cache[info.name] = (time.monotonic(), _pane_status(pane))
                # Mesma captura serve o radar de limite: assim a travada raramente paga a sua.
                self._limit_cache[info.name] = (time.monotonic(), rate_limit_reset(pane))
                # Spinner da MESMA captura (classify e puro/regex): e o que devolve a barrinha de
                # "trabalhando" pro card quando o estado veio do marcador (que nao traz label).
                # SO grava se a captura PARECE working — captura unica nao distingue spinner vivo
                # de marcador congelado no scrollback (o caminho pending faz captura dupla pra
                # isso; aqui dobrar o fork nao vale — pane nao-working derruba o label e o proximo
                # sweep re-avalia).
                st_c, lbl_c = classify(pane)[:2]
                if st_c == "working":
                    self._label_cache[info.name] = lbl_c
                else:
                    self._label_cache.pop(info.name, None)
            except Exception as e:
                # tmux engasgado (transiente): carimba o relogio (senao o nome quebrado monopolizaria
                # o budget a cada chamada) mas PRESERVA a ultima statusline boa — apagar aqui piscava
                # o badge do card e forcava re-emissao da lista a toa. Sessao morta de verdade sai da
                # lista sozinha (e kill via app limpa no _forget). Logado em debug: tmux quebrado
                # cronico deixaria toda statusline congelada sem rastro nenhum.
                _log.debug("statusline capture falhou pra %s: %r", info.name, e)
                prev = self._status_cache.get(info.name, (0.0, None))[1]
                self._status_cache[info.name] = (time.monotonic(), prev)
                # Label NAO segue o preserve do status_line: statusline velha ainda e verdadeira
                # (modelo/custo mudam devagar); spinner velho vira fantasma — melhor sem barrinha.
                self._label_cache.pop(info.name, None)
        # Codex: a linha sai do PROPRIO rollout (modelo, contexto e cota estao la), com o mesmo
        # cache e o mesmo TTL do sweep de pane — o card nao tem SSE aberto, e raspar a TUI esta
        # proibido. SEM o `_STATUS_BUDGET` de proposito: aquele teto existe pra limitar FORKS de
        # `capture-pane`, e aqui e leitura do fim de um arquivo no threadpool.
        # O ticket pedia isto "pelo sidecar de status". Nao ha sidecar: quem escreveria seria este
        # mesmo processo, lendo este mesmo arquivo — o sidecar existe pra que QUEM RENDERIZA
        # publique o que so ele sabe (ver app/statusline.py), e aqui o backend sabe tudo. Gravar
        # um arquivo pra ler de volta seria so um passo a mais entre a mesma fonte e o mesmo card.
        codexes = [i for i in infos
                   if getattr(i, "provider", "claude") == "codex" and i.jsonl
                   and now_m - self._status_cache.get(i.name, (0.0, None))[0] > _STATUS_TTL]
        if codexes:
            linhas = await asyncio.gather(*[
                asyncio.to_thread(_codex_status_line, i.jsonl) for i in codexes])
            for info, linha in zip(codexes, linhas):
                # Linha nova ou a ULTIMA BOA: preservar segue a regra do sweep de pane — apagar
                # piscaria o badge do card e forcaria re-emissao da lista a toa.
                anterior = self._status_cache.get(info.name, (0.0, None))[1]
                self._status_cache[info.name] = (time.monotonic(), linha or anterior)
        for info in infos:
            if getattr(info, "provider", "claude") == "codex":
                info.status_line = self._status_cache.get(info.name, (0.0, None))[1]
            elif getattr(info, "headless", False):
                # Já veio do adapter (processo vivo) ou do sidecar do hook; nunca do pane.
                info.status_line = info.status_line or _sidecar_status(_sid(info.jsonl))
            else:
                # Sidecar antes do pane: a captura traz a linha ja CORTADA na largura da janela
                # (quem renderiza trunca antes de imprimir, ver app/statusline.py). Ler o arquivo e
                # muito mais barato que a captura — nao entra no budget de forks acima.
                info.status_line = (_sidecar_status(_sid(info.jsonl))
                                    or self._status_cache.get(info.name, (0.0, None))[1])
                # So preenche o buraco do fast-path: quem foi classificada pelo pane ja tem label
                # fresco (e idle de verdade fica sem label — o cache so vale se ainda working).
                # getattr: fakes de teste nao tem o campo.
                if info.state == "working" and getattr(info, "label", None) is None:
                    info.label = self._label_cache.get(info.name)
        # Contexto das sessoes Claude pelo transcript, que nao depende de a statusline ser a do
        # Hangar. Mesmo TTL da statusline: e leitura do fim de um arquivo, no threadpool.
        claudes = [i for i in infos
                   if getattr(i, "provider", "claude") == "claude" and i.jsonl
                   and (i.name not in self._context_cache
                        or self._context_cache[i.name][2] is None
                        or now_m - self._context_cache[i.name][0] > _STATUS_TTL
                        or self._context_cache[i.name][1] != i.jsonl)]
        if claudes:
            versions = [claude_context.source_version(i.jsonl) for i in claudes]
            lidos = await asyncio.gather(*[
                asyncio.to_thread(_claude_reading, i, self._agent_pid.get(i.name))
                for i in claudes])
            for info, (ctx, model), version in zip(claudes, lidos, versions):
                _, jsonl_antes, anterior, modelo_antes = self._context_cache.get(info.name, (0.0, None, None, None))
                # Sem resposta lida, o valor anterior só vale para o MESMO transcript; o modelo também,
                # senão a pílula cai no da conta quando a resposta sai do trecho lido.
                mesmo = jsonl_antes == info.jsonl
                manter = anterior if mesmo else None
                if ctx is None and mesmo and modelo_antes:
                    model = modelo_antes
                # Uma publicação durante o threadpool não pode carimbar a leitura antiga como atual.
                at = time.monotonic()
                if version != claude_context.source_version(info.jsonl):
                    at -= _STATUS_TTL + 1
                self._context_cache[info.name] = (at, info.jsonl, ctx or manter, model)
        for info in infos:
            if getattr(info, "provider", "claude") == "claude":
                _, jsonl_lido, ctx, model = self._context_cache.get(info.name, (0.0, None, None, None))
                info.context = ctx if jsonl_lido == info.jsonl else None
                info.model = model if jsonl_lido == info.jsonl else None
        # Travada (feature #7): "working" ha mais de CP_STALL_SECONDS sem o transcript avancar. So o
        # bool derivado pra UI/sig — o push (1x, com dedupe) e responsabilidade do stall_watch, nao daqui.
        now = time.time()
        for info in infos:
            info.stalled = (
                info.state == "working"
                and info.last_activity is not None
                and (now - info.last_activity) > runtime_config.get("stall_seconds")
            )
        await self._radar_de_limite(infos, raspadas={i.name for i in pending})
        if state_only:
            return infos
        # Última resposta para a linha parada da lista. Usa o mesmo tail-read provider-aware do
        # /history e roda fora do event loop; working/awaiting continuam mostrando o sinal vivo.
        def _decorate_replies() -> None:
            for info in infos:
                info.last_reply = None
                info.last_reply_at = None
                if info.state != "idle" or not info.jsonl:
                    continue
                provider = getattr(info, "provider", "claude")
                marker = (info.jsonl, info.last_activity, provider)
                cached = self._reply_cache.get(info.name)
                if cached is None or cached[:3] != marker:
                    try:
                        event = next((ev for ev in reversed(merged_history(
                            info.name, info.jsonl, provider, limit=8
                        )) if ev.kind == "assistant_msg" and ev.text), None)
                    except Exception:
                        _log.warning("lista: falha lendo ultima resposta sessao=%s", info.name,
                                     exc_info=True)
                        continue
                    text = _texto_simples(event.text)[:160] if event and event.text else None
                    cached = (*marker, text, (event.ts or info.last_activity) if event else None)
                    self._reply_cache[info.name] = cached
                info.last_reply, info.last_reply_at = cached[3], cached[4]

        await asyncio.to_thread(_decorate_replies)
        # Estado de git por sessão — SÓ aqui (payload do /api/sessions), nunca em list(): git_summary
        # forka `git status` e list() é o caminho leve chamado por kill()/resume/SSE. E como
        # list_with_state é awaitado direto no event loop (/api/sessions, sse, stall_watch), o loop
        # de forks vai pro threadpool via asyncio.to_thread — rodar na corrotina congelaria o backend
        # inteiro no cache-miss. Gate em .git e except GitError moram no git_summary; cache de 3s
        # segura o custo vs o poll de 2s.
        # O git NAO segura a lista: ela sai com o ultimo numero conhecido por cwd e o git atualiza
        # em segundo plano, um por repositorio (single-flight). Em serie, um repositorio lento
        # atrasava o card de TODAS as sessoes — inclusive as que acabaram de mudar de estado.
        # O git é o de onde o agente trabalha: numa worktree, a pasta de abertura não diz nada.
        for info in infos:
            summary, diffstat = _git_ultimo.get(_git_dir(info), (None, None))
            if summary is not None:
                info.git_dirty = summary["dirty"]
                info.git_ahead = summary["ahead"]
                info.git_behind = summary["behind"]
            if diffstat is not None:
                info.git_added = diffstat["added"]
                info.git_removed = diffstat["removed"]
        for cwd in {d for d in map(_git_dir, infos) if d}:
            if cwd not in _git_em_voo:
                _git_em_voo.add(cwd)
                asyncio.create_task(_atualizar_git(cwd))

        def _decorate_planos() -> None:
            # Le markdown do disco: ler arquivo na corrotina e a mesma classe de erro que motivou
            # o to_thread do git.
            for info in infos:
                _decorate_plan(info)

        await asyncio.to_thread(_decorate_planos)
        for info in infos:
            _decorate_loop(info)
        await decorate_access(infos + orqs + transfers)
        return infos + orqs + transfers

    @diag.rastrear("sessao.criar")
    def create(self, name: str, cwd: str, config_dir: str | None = None,
               resume_session_id: str | None = None, provider: str = "claude",
               engine: str | None = None, model: str | None = None,
               effort: str | None = None, context_window: int | None = None,
               permission_mode: str | None = None,
               initial_prompt: str | None = None,
               omp_profile: str | None = None,
               codex_account: str | None = None,
               read_only: bool = False,
               headless: bool = False,
               subagent_model: str | None = None,
               jev: bool = False, transfer_id: str | None = None,
               tool_output_token_limit: int | None = None,
               transfer_rollout_path: str | None = None,
               engine_account: str | None = None, engine_models: list[dict] | None = None,
               service_tier: str | None = None,
               claude_customizations: dict | None = None) -> SessionInfo:
        if claude_customizations is not None and provider != "claude":
            raise ValueError("plugins e skills por sessão só valem para Claude")
        if service_tier is not None:
            if service_tier not in ("default", "priority"):
                raise ValueError("service_tier: use default ou priority")
            if provider != "codex":
                from app import cliproxy
                if provider != "claude" or not cliproxy.supports_fast(engine, model):
                    raise ValueError("service_tier exige Codex ou Claude com motor GPT no CLIProxyAPI local")
        # Nome tmux nao aceita "."/":"/espaco -> sanitiza igual ao rename. Varias sessoes na MESMA
        # pasta sao permitidas: cada uma tem nome unico + --session-id proprio -> jsonl proprio.
        name = sanitize_session_name(name)
        diag.registrar("sessao.criar_etapa", sessao=name, provider=provider, etapa="validar")
        if not name:
            raise ValueError("nome invalido")
        from app.conversation_transfer import require_available
        require_available(name)
        fixed_account = None
        if engine_account is not None:
            from app import cliproxy, engines
            if provider != "claude" or not engine:
                raise ValueError("conta ChatGPT exige Claude com motor CLIProxyAPI local")
            fixed_account = cliproxy.account_for_engine(engines.listar().get(engine, {}), engine_account)
            binding = cliproxy.engine_env(engine, model, context_window, engine_account,
                                         home=fixed_account["home"], models=engine_models)
            model = binding["ANTHROPIC_MODEL"]
            context_window = int(binding["CLAUDE_CODE_MAX_CONTEXT_TOKENS"]) if binding.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS") else None
        if subagent_model is not None:
            if provider != "claude" or engine:
                raise ValueError("modelo dos subagentes so vale para claude sem motor")
            model_args.validar("claude", subagent_model, None)
        if provider == "claude" and permission_mode is None:
            # "padrão" na tela vira o modo da conta AQUI, não lá no arranque: assim a sessão nasce
            # no modo que o app mostra, na máquina que define `defaultMode` e na que não define.
            permission_mode = modo_permissao.modo_da_conta(config_dir)
        if transfer_rollout_path is not None and transfer_id is None:
            raise ValueError("session_transfer_invalid_record")
        if transfer_id is not None:
            from app.conversation_transfer import load_transfer, TransferPhase
            from app.conversation_history import source_rows, verify_boundary
            record = load_transfer(transfer_id)
            target = record.destination_meta if record else None
            if (provider != "codex" or record is None or record.phase != TransferPhase.COMPLETE
                    or not target or target.get("thread_id") != resume_session_id
                    or target.get("codex_account") != (codex_account or "default")):
                raise ValueError("session_transfer_invalid_record")
            source_rows(record.source)
            transfer_rollout_path = transfer_rollout_path or target["rollout_path"]
            verify_boundary(record, transfer_rollout_path)
            if tool_output_token_limit != target.get("tool_output_token_limit"):
                raise ValueError("session_transfer_invalid_record")
            model = model if model is not None else target.get("model")
            effort = effort if effort is not None else target.get("effort")
            permission_mode = permission_mode if permission_mode is not None else target.get("permission_mode")
        self._leave_old_group(name)
        if headless:
            if provider not in ("claude", "codex"):
                raise ValueError("sessao sem terminal so vale para provider claude ou codex")
            if read_only or initial_prompt:
                raise ValueError("sessao sem terminal nao aceita read_only nem prompt inicial")
            if provider == "codex":
                if engine:
                    raise ValueError("motor so vale para provider claude")
                return self._create_codex_headless(name, cwd, resume_session_id, model, effort,
                                                   permission_mode, codex_account, jev,
                                                   transfer_id, tool_output_token_limit, transfer_rollout_path,
                                                   service_tier=service_tier)
            return self._create_headless(name, cwd, config_dir, resume_session_id, engine, model,
                                         effort, context_window, permission_mode, subagent_model,
                                         jev, engine_account,
                                         fixed_account["credential_id"] if fixed_account else None,
                                         fixed_account["base_url"] if fixed_account else None,
                                         service_tier=service_tier, claude_customizations=claude_customizations)
        claude_settings = None
        codex_home = None
        if provider == "codex":
            try:
                account = codex_contas.resolve_account(codex_account or "default")
            except codex_contas.AccountError as exc:
                raise ValueError(f"{exc.code}: {exc.params}") from None
            codex_home = str(account.home.expanduser().absolute())
        elif codex_account is not None:
            raise ValueError("codex_account so vale para provider codex")
        protected_prefix = []
        if read_only:
            from app.orq_readonly import prepare
            protected_prefix = prepare(cwd, runtime_dirs=(config_dir or "", codex_home or ""))
        if omp_profile:
            if provider != "omp":
                raise ValueError("perfil so vale para provider omp")
            # A MESMA regra de nome do omp (resolve_omp_directories): o valor vai pro ambiente do pane.
            from app.omp_plugin_sync import InventoryError, resolve_omp_directories
            try:
                resolve_omp_directories(Path.home(), {"OMP_PROFILE": omp_profile}, Path.home())
            except InventoryError as e:
                raise ValueError(str(e)) from None
        # Motor de modelo: valida ANTES de criar o pane. Motor inexistente com env vazio faria a
        # sessão subir na conta Anthropic ACHANDO que é o motor pedido — falha silenciosa.
        if engine:
            from app import engines
            if engine not in engines.listar():
                raise ValueError(f"motor '{engine}' nao existe")
        # Pi anda no MESMO caminho tmux do Claude, mas duas coisas daqui pra baixo sao Claude puro e
        # recusam alto em vez de "quase funcionar":
        #  - motor: o `hangar-engine --exec` so exporta ANTHROPIC_* / CLAUDE_CODE_*, que o pi ignora ->
        #    a sessao subiria na conta do proprio pi PARECENDO estar no motor pedido.
        # Resume do Pi passou a existir (branch `elif provider == "pi"` la embaixo, com
        # `pi --session-id <id>`); a recusa que morava aqui tornava aquele branch INALCANCAVEL.
        if provider in ("pi", "omp") and engine:
            raise ValueError("motor so vale para provider claude")
        # Kimi anda no MESMO caminho tmux do Pi. Motor segue Claude-puro (hangar-engine so exporta
        # ANTHROPIC_*). Resume existe: `kimi --session <id>` (diferente do Pi, que nao tinha flag).
        if provider == "kimi" and engine:
            raise ValueError("motor so vale para provider claude")
        # Codex idem: o `hangar-engine --exec` so exporta ANTHROPIC_*/CLAUDE_CODE_*, que o codex
        # ignora — a sessao subiria na conta do proprio Codex PARECENDO estar no motor pedido.
        if provider == "codex":
            if engine:
                raise ValueError("motor so vale para provider claude")
            _exigir_lancador_codex()
        # Skill instalada no Claude vale JA nesta sessao. pi, kimi e codex leem so as pastas deles;
        # quem materializa as skills do Claude la e a ponte, e ela era refeita apenas na subida do
        # backend — instalar uma skill exigia reiniciar o servico. Claude e omp ficam de fora
        # porque os dois descobrem as fontes sozinhos. Fail-soft: criar sessao nunca depende disto.
        if provider in ("pi", "kimi"):
            try:
                from app import skill_bridge
                # Silencioso no caso comum (nada mudou) e falante quando MEXEU: sem a segunda
                # metade, "a ponte rodou e achou pouco" e "a ponte nem rodou" voltam a ser o mesmo
                # silencio no diario — que foi a duvida que sobrou quando 67 skills sumiram.
                stats = skill_bridge.rebuild(log=lambda _m: None)
                mexeu = {n: s for n, s in stats.items()
                         if any(s.get(k) for k in ("criados", "trocados", "removidos", "erro"))}
                if mexeu:
                    _log.info("ponte de skills antes de criar %s: %s", name, mexeu)
            except Exception:                          # noqa: BLE001
                _log.warning("ponte de skills falhou antes de criar %s", name, exc_info=True)
        # Unicidade contra tmux (Claude) E sidecars Codex: sem o segundo check, um nome de sessao
        # Codex reusado aqui geraria DOIS SessionInfo com o mesmo name no list() (front keyed por
        # nome) e o kill(name) cairia no branch Codex (checado 1o) -> fecharia o client Codex sem
        # matar o pane tmux (pane orfao inkillavel).
        if tmux.has_session(name) or codex_sessions.exists(name) or headless_sessions.exists(name):
            diag.registrar("sessao.criar_recusada", "aviso", sessao=name, provider=provider,
                           detalhe="nome_ja_em_uso")
            raise ValueError("ja existe uma sessao com esse nome")
        diag.registrar("sessao.criar_etapa", sessao=name, provider=provider, etapa="preparar_comando")
        # resume_session_id (retomar conversa MORTA do Arquivo): reusa o uuid existente e sobe com
        # `--resume` em vez de `--session-id` -> o claude CONTINUA aquele jsonl (nao comeca um novo).
        # Mesmo uuid ja validado no endpoint, mas revalida aqui tambem (vai direto pro comando do shell).
        # Os quatro providers retomam por aqui, cada um com o comando DELE: `claude --resume`,
        # `pi --session-id`, `kimi --session`, e o lancador com `codex resume`.
        if resume_session_id is not None:
            if provider == "kimi":
                # Sid do Kimi e `session_<uuid>` (nao uuid puro) e o resume e `--session`, nao
                # --resume. Validacao propria: vai direto pro comando do shell.
                if not re.fullmatch(r"session_[0-9a-fA-F-]{36}", resume_session_id):
                    raise ValueError("session_id invalido")
                sid = resume_session_id
                # args_de("kimi", None, None) == []: a escolha na abertura nao cobre o Kimi (e
                # qualquer escolha pra ele e recusada pela validacao na api), e com ausencia o
                # join_cmd e byte por byte o f-string de antes (no POSIX ele E o shlex.join).
                cmd = tmux.join_cmd(["kimi", "--session", sid]
                                 + model_args.args_de(provider, model, effort))
            elif provider == "codex":
                # O id da conversa Codex e o uuid do fim do nome do rollout (o mesmo que o Arquivo
                # lista). Vai pro lancador, que abre a TUI com `codex resume <id>` — o historico ja
                # esta no rollout, entao a sessao nova nasce com a conversa inteira.
                # O MESMO criterio que a rota do Arquivo usa (uma definicao só de id de conversa).
                from app.archive_providers import UUID_RE
                if not UUID_RE.match(resume_session_id):
                    raise ValueError("session_id invalido")
                sid = resume_session_id
                from app.adapters.codex.lancador import comando_do_lancador
                cmd = tmux.join_cmd(comando_do_lancador(cwd, thread_id=sid,
                                                       codex_home=codex_home,
                                                       codex_account=account.id,
                                                       model=model, effort=effort,
                                                       tool_output_token_limit=tool_output_token_limit,
                                                       **({"service_tier": service_tier} if service_tier is not None else {})))
            elif provider == "omp":
                # Retoma por CAMINHO: o id interno do omp nao e o do nome do arquivo, e spawn e
                # resume sao verbos diferentes — reusar o spawn abriria conversa nova.
                try:
                    uuid.UUID(resume_session_id)
                except (ValueError, AttributeError, TypeError):
                    raise ValueError("session_id invalido")
                sid = resume_session_id
                from app.adapters import get_adapter
                cmd = tmux.join_cmd(get_adapter("omp").resume_command(cwd, sid, model, effort, omp_profile))
            elif provider == "pi":
                # `pi --session-id <id>` RETOMA quando o id ja existe ("creating it if missing", no
                # --help do 0.82.1) -> o comando do resume e o mesmo do spawn, so com o id antigo.
                # Sem este ramo o Pi caia no `claude --resume` abaixo e o pane subia o agente errado.
                try:
                    uuid.UUID(resume_session_id)
                except (ValueError, AttributeError, TypeError):
                    raise ValueError("session_id invalido")
                sid = resume_session_id
                from app.adapters import get_adapter
                cmd = tmux.join_cmd(get_adapter("pi").spawn_command(cwd, sid, model, effort, None))
            else:
                try:
                    uuid.UUID(resume_session_id)
                except (ValueError, AttributeError, TypeError):
                    raise ValueError("session_id invalido")
                sid = resume_session_id
                # Retomada de conversa MORTA (vinda do Arquivo): nao existe sessao antiga nem pid
                # pra consultar. A escolha que vale e a que este create() recebeu.
                # permission_mode NÃO entra no resume (a sessão retoma no estado dela).
                claude_settings = session_customizations.prepare(
                    sid, cwd, tmux.config_dir_de(config_dir), claude_customizations, resume=True)
                from app.adapters.claude import terminal_command
                cmd = tmux.join_cmd(terminal_command(
                    cwd, sid, model, effort, claude_settings=claude_settings, resume=True))
        else:
            sid = str(uuid.uuid4())
            if provider == "claude":
                claude_settings = session_customizations.prepare(
                    sid, cwd, tmux.config_dir_de(config_dir), claude_customizations, resume=False)
            # spawn_command vem do Adapter do provider (import local: get_adapter->ClaudeAdapter nao
            # importa registry, mas evita qualquer ciclo se um adapter futuro vier a importar daqui).
            from app.adapters import get_adapter
            # initial_prompt so vai pro Codex: e a TUI dele que abre a thread, entao o 1o prompt tem
            # que estar no comando do pane. Os outros providers recebem prompt inicial por /input,
            # e aceitar o argumento neles seria escolha que some calada.
            extra = {"initial_prompt": initial_prompt} if provider == "codex" else {}
            if claude_settings is not None:
                extra["claude_settings"] = claude_settings
            if provider == "codex":
                extra["codex_home"] = codex_home
                extra["codex_account"] = account.id
                if service_tier is not None:
                    extra["service_tier"] = service_tier
            if provider == "omp" and omp_profile:
                extra["perfil"] = omp_profile
            cmd = tmux.join_cmd(get_adapter(provider).spawn_command(
                cwd, sid, model, effort, permission_mode, **extra))
        if engine:
            # `hangar-engine --exec` aplica o env DENTRO do pane (os.execvpe). Não usamos `tmux -e` porque
            # a key ficaria em /proc/<pid>/cmdline, legível por qualquer usuário da máquina. Depois do
            # exec o cmdline é o do claude, então a resolução de transcript por --session-id/--resume
            # continua funcionando.
            #
            # Modelo e janela entram NO PREFIXO, não só no comando: o env que o hangar-engine aplica
            # exporta o mesmo modelo em cinco chaves e a janela em outra — a flag sozinha ganharia só
            # de ANTHROPIC_MODEL (ver engines.env_de). A janela vem do catálogo do provedor, resolvida
            # no backend (api.create_session); quem passa por aqui sem ela (ex: resume do Arquivo)
            # simplesmente não exporta a var — o CLI usa o default dele.
            _exigir_cp_engine()
            pre = ["hangar-engine", "--exec", engine]
            if engine_account is not None:
                pre += ["--account", engine_account, "--account-home", fixed_account["home"],
                        "--account-base-url", fixed_account["base_url"]]
            if model:
                pre += ["--model", model]
                if context_window:
                    pre += ["--context", str(context_window)]
            if service_tier is not None:
                pre += ["--service-tier", service_tier]
            cmd = tmux.join_cmd(pre + ["--"]) + " " + cmd
        base = (Path(config_dir) / "projects") if config_dir else self.projects_dir
        # Pi tem layout PROPRIO (~/.pi/agent/sessions/<slug>/<ts>_<uuid>.jsonl) e o arquivo so nasce
        # quando a TUI grava o 1o turno -> nao ha path pra pre-semear. jsonl=None e cache INTOCADO
        # (ver o final do metodo): o _jsonl_cache e de CLASSE, compartilhado com o sse, e um path do
        # layout do Claude ali seria um arquivo que nunca existe, devolvido por resolve() pra sempre.
        # Quem liga o pane ao transcript e o bilhete que a extensao escreve (ver pi_session_file).
        # Kimi idem (sessions/<wd>/session_<uuid>/agents/main/wire.jsonl, sessao so no 1o prompt);
        # quem liga e o bilhete do hook (ver kimi_session_file).
        # Codex pelo mesmo motivo: o rollout so existe depois que a TUI abre a thread, e o caminho
        # dele nao se deriva de cwd+id (vem do thread/start). Devolver um path do layout do Claude
        # aqui envenenaria o _jsonl_cache, que e de CLASSE e compartilhado com o SSE.
        jsonl = None if provider in ("pi", "omp", "kimi", "codex") else str(base / sanitize_cwd(cwd) / f"{sid}.jsonl")
        # Pré-confia a pasta no .claude.json: sem isto, uma sessão criada pelo app numa pasta NOVA
        # nasce presa no "trust this folder?" do Claude Code (invisível/ininteragível pelo chat até
        # aceitar na TUI). Só é o 1º acesso à pasta — depois o próprio Claude Code grava. Best-effort.
        # Pi não lê o .claude.json e tem o próprio fluxo de confiança -> escrever ali só sujaria a
        # lista de pastas confiadas do Claude com pasta que ele talvez nunca abra.
        # Kimi tem trust PROPRIO (medido: pasta nova trava no "Trust this folder?" do boot) ->
        # pré-confia no formato dele (~/.kimi-code/workspace-trust), nao no do Claude.
        # A conta Codex secundaria pre-confia pelo endpoint de preparo chamado DENTRO do launcher:
        # assim termina a sincronizacao antes de qualquer escrita no config e antes da TUI. A
        # padrao nao tem esse preparo e continua pre-confiando aqui, antes de criar o pane.
        diag.registrar("sessao.criar_etapa", sessao=name, provider=provider, etapa="confiar_pasta")
        if provider == "kimi":
            from app.adapters.kimi import sessions as kimi_sessions
            kimi_sessions.pretrust_cwd(cwd)
        elif provider == "codex" and account.is_default and transfer_id is None:
            codex_sessions.pretrust_cwd(cwd, codex_home=codex_home)
        elif provider not in ("pi", "omp", "codex"):
            _pretrust_cwd(cwd, config_dir)
        if protected_prefix:
            cmd = tmux.join_cmd([*protected_prefix, "/bin/sh", "-c", cmd])
        diag.registrar("sessao.criar_etapa", sessao=name, provider=provider, etapa="criar_terminal")
        self._forget(name, required=True)
        # Sessao NOVA = sid novo = transcript fresco. A fila duravel e keyed pelo NOME (sobrevive ao
        # fim da sessao antiga), entao entradas remanescentes de uma sessao morta de mesmo nome
        # fantasmariam aqui via merged_history. Limpa ANTES do pane: depois dele o runtime ja adota
        # a sessao nova, e a limpeza esbarrava na posse em transferencia.
        _retire_waiting_runtime(name)
        PromptQueue(name).clear()
        env_pane = _env_sessao(subagent_model, jev, provider, nome=name, claude_settings=claude_settings)
        if transfer_id is not None:
            key = uuid.uuid4().hex
            codex_sessions.save(name, sid, transfer_rollout_path, cwd, model=model, effort=effort,
                                codex_home=codex_home, codex_account=account.id, key=key,
                                transfer_id=transfer_id, tool_output_token_limit=tool_output_token_limit,
                                permission_mode=permission_mode, previous_non_plan=target.get("previous_non_plan"),
                                launching=True)
            env_pane.setdefault("env", {})["CP_SESSION_KEY"] = key
        if not tmux.new_session(name, cwd, cmd, config_dir, provider=provider, **env_pane):
            if transfer_id is not None and not tmux.has_session(name):
                current = codex_sessions.load(name)
                if current and current.get("key") == key and current.get("launching"):
                    codex_sessions.delete(name)
            diag.registrar("sessao.criar_recusada", "erro", sessao=name, provider=provider,
                           detalhe="terminal_nao_criado")
            raise ValueError("falha ao criar sessao no tmux")
        diag.registrar("sessao.criar_etapa", sessao=name, provider=provider, etapa="limpar_estado_anterior")
        # Mesmo motivo da fila, pro vinculo 'then' (feature #12): nome reusado nao deve herdar um encadeamento
        # de uma sessao antiga e ja morta.
        ThenLink(name).clear()
        # E pro PAREAMENTO, pelo mesmo motivo: o kill() ja tira a sessao do grupo, mas quem morre
        # FORA dele (pane fechado na mao, maquina reiniciada) deixa o sidecar keyed pelo nome no
        # disco — e a sessao nova de mesmo nome nascia dentro de um grupo que nao existe mais.
        # Nome reusado não herda o par externo da sessão antiga.
        _encerrar_pares_externos(name)
        _log_leave_warnings(name, self._clear_pair(name))
        # Fixa o jsonl FRESCO no cache na hora: resolve() devolve este uuid mesmo antes do claude
        # escrever o arquivo, evitando o fallback newest-by-mtime pescar um jsonl ja existente da pasta.
        # Pi (jsonl=None) nao entra no cache — nao ha path a fixar, e a resolucao dele nem passa por aqui.
        if jsonl is not None:
            self._seed(name, jsonl)
        diag.registrar("sessao.criada", sessao=name, provider=provider, etapa="terminal_criado")
        return SessionInfo(name=name, cwd=cwd, jsonl=jsonl, tracked=jsonl is not None,
                           provider=provider, engine=engine, engine_account=engine_account,
                           conta=fixed_account["credential_id"] if fixed_account else None,
                           codex_home=codex_home)

    def _create_headless(self, name: str, cwd: str, config_dir: str | None,
                         resume_session_id: str | None, engine: str | None, model: str | None,
                         effort: str | None, context_window: int | None,
                         permission_mode: str | None, subagent_model: str | None = None,
                         jev: bool = False, engine_account: str | None = None,
                         engine_credential_id: str | None = None,
                         engine_account_base_url: str | None = None,
                         service_tier: str | None = None,
                         claude_customizations: dict | None = None) -> SessionInfo:
        """Sessão Claude SEM terminal: criar é gravar o sidecar. O processo `claude` sobe no
        primeiro prompt (e de novo, com --resume, depois de um restart do backend) — abrir a
        sessão não custa um processo, e nada aqui depende de tmux."""
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        if engine:
            from app import engines
            if engine not in engines.listar():
                raise ValueError(f"motor '{engine}' nao existe")
            _exigir_cp_engine()
        if tmux.has_session(name) or codex_sessions.exists(name) or headless_sessions.exists(name):
            diag.registrar("sessao.criar_recusada", "aviso", sessao=name, provider="claude",
                           detalhe="nome_ja_em_uso")
            raise ValueError("ja existe uma sessao com esse nome")
        if resume_session_id is not None:
            try:
                uuid.UUID(resume_session_id)
            except (ValueError, AttributeError, TypeError):
                raise ValueError("session_id invalido")
            sid = resume_session_id
        else:
            sid = str(uuid.uuid4())
        model_args.validar("claude", model, effort, permission_mode)
        claude_settings = session_customizations.prepare(
            sid, cwd, tmux.config_dir_de(config_dir), claude_customizations,
            resume=resume_session_id is not None)
        diag.registrar("sessao.criar_etapa", sessao=name, provider="claude", etapa="confiar_pasta")
        _pretrust_cwd(cwd, config_dir)
        self._forget(name, required=True)
        # Antes do sidecar: depois dele o runtime pode adotar a sessão nova no meio da limpeza.
        _retire_waiting_runtime(name)
        PromptQueue(name).clear()
        # Nascer JÁ no plano deixaria a sessão sem modo de base: é ele que diz pra onde
        # "Implementar o plano" volta e se o plano precisa perguntar por ferramenta.
        anterior = modo_permissao.modo_da_conta(config_dir) if permission_mode == "plan" else None
        meta = headless_sessions.save(name, cwd, sid, config_dir=config_dir, engine=engine,
                                      model=model, effort=effort, context_window=context_window,
                                      permission_mode=permission_mode, previous_non_plan=anterior,
                                      subagent_model=subagent_model, jev=jev,
                                      engine_account=engine_account,
                                      engine_credential_id=engine_credential_id,
                                      engine_account_base_url=engine_account_base_url,
                                      service_tier=service_tier, claude_settings=claude_settings)
        ThenLink(name).clear()
        # Nome reusado não herda o par externo da sessão antiga.
        _encerrar_pares_externos(name)
        _log_leave_warnings(name, self._clear_pair(name))
        jsonl = get_adapter(CLAUDE_HEADLESS).transcript_path_de(meta)
        self._seed(name, jsonl)
        diag.registrar("sessao.criada", sessao=name, provider="claude", etapa="sidecar_gravado")
        return SessionInfo(name=name, cwd=cwd, jsonl=jsonl, tracked=True, provider="claude",
                           headless=True, engine=engine, engine_account=engine_account,
                           conta=engine_credential_id if engine_account else None)

    def _create_codex_headless(self, name: str, cwd: str, resume_thread_id: str | None,
                               model: str | None, effort: str | None, permission_mode: str | None,
                               codex_account: str | None, jev: bool = False,
                               transfer_id: str | None = None,
                               tool_output_token_limit: int | None = None,
                               transfer_rollout_path: str | None = None,
                               service_tier: str | None = None) -> SessionInfo:
        """Sessão Codex SEM terminal: grava o sidecar; o app-server sobe no cano logo em seguida
        pelo `watch_sessions` do adapter (aquece na criação, não no primeiro prompt)."""
        from app.adapters.codex import sem_terminal
        try:
            account = codex_contas.resolve_account(codex_account or "default")
        except codex_contas.AccountError as exc:
            raise ValueError(f"{exc.code}: {exc.params}") from None
        codex_home = str(account.home.expanduser().absolute())
        if tmux.has_session(name) or codex_sessions.exists(name) or headless_sessions.exists(name):
            diag.registrar("sessao.criar_recusada", "aviso", sessao=name, provider="codex",
                           detalhe="nome_ja_em_uso")
            raise ValueError("ja existe uma sessao com esse nome")
        model_args.validar("codex", model, effort)
        if permission_mode is not None and not any(
                m[0].lower() == permission_mode.strip().lower() for m in sem_terminal.MODOS):
            raise ValueError("permission_mode: use um de " + ", ".join(m[0] for m in sem_terminal.MODOS))
        diag.registrar("sessao.criar_etapa", sessao=name, provider="codex", etapa="confiar_pasta")
        if transfer_id is None:
            codex_sessions.pretrust_cwd(cwd, codex_home=codex_home)
        self._forget(name, required=True)
        target = {}
        if transfer_id:
            from app.conversation_transfer import load_transfer
            target = load_transfer(transfer_id).destination_meta
        rollout = transfer_rollout_path or target.get("rollout_path") or (sem_terminal.rollout_de(resume_thread_id, codex_home) if resume_thread_id else "")
        # Antes do sidecar: depois dele o runtime pode adotar a sessão nova no meio da limpeza.
        _retire_waiting_runtime(name)
        PromptQueue(name).clear()
        codex_sessions.save(name, resume_thread_id, rollout, cwd, model=model, effort=effort,
                            codex_home=codex_home, codex_account=codex_account,
                            headless=True, key=sem_terminal.nova_chave(), permission_mode=permission_mode,
                            jev=jev, transfer_id=transfer_id,
                            tool_output_token_limit=tool_output_token_limit,
                            previous_non_plan=target.get("previous_non_plan"), service_tier=service_tier)
        ThenLink(name).clear()
        # Nome reusado não herda o par externo da sessão antiga.
        _encerrar_pares_externos(name)
        _log_leave_warnings(name, self._clear_pair(name))
        diag.registrar("sessao.criada", sessao=name, provider="codex", etapa="sidecar_gravado")
        return SessionInfo(name=name, cwd=cwd, jsonl=rollout or None, tracked=True, provider="codex",
                           headless=True, conta=f"codex:{codex_home}", codex_home=codex_home)

    # ── Troca terminal ⇄ sem terminal (mesma conversa) ──────────────────────────────────────
    # É troca, não cópia: o antigo morre antes do novo nascer, e fila, `then` e pareamento ficam
    # (são da sessão, não do transporte). Quem garante que a sessão está ociosa é a API.

    def para_terminal(self, name: str, *, engine_models: list[dict] | None = None) -> SessionInfo:
        """Sessão sem terminal vira pane tmux com `claude --resume`. Falhando o pane, o sidecar
        volta e quem chama religa o processo — a sessão nunca fica sem nenhum dos dois."""
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        meta = headless_sessions.load(name)
        if meta is None:
            raise ValueError("sessao sem terminal nao encontrada")
        hl = get_adapter(CLAUDE_HEADLESS)
        jsonl = hl.transcript_path_de(meta)
        escolha = meta
        if not meta.get("engine"):
            # O processo sabe o modelo e o esforço em uso; o sidecar só guarda o que foi pedido.
            vivo_m, vivo_e = hl.escolhas(name)
            escolha = {**meta, "model": vivo_m or meta.get("model"),
                       "effort": _esforco_de_abertura(vivo_e) or meta.get("effort")}
        # Comando inteiro ANTES de matar: validação que estoura depois deixaria a sessão sem nada.
        cmd = self._comando_terminal(escolha, resume=Path(jsonl).exists(), engine_models=engine_models)
        headless_sessions.marcar_troca(name)
        headless_sessions.delete(name)
        try:
            hl.close_sync(name, meta)
        except Exception:
            headless_sessions.restaurar(meta, preserve_process=True)
            raise
        cano_pid = (meta.get("cano") or {}).get("pid")
        _esperar_saida([int(cano_pid)] if cano_pid else [])
        self._forget(name)
        if not tmux.new_session(name, meta["cwd"], cmd, meta.get("config_dir"), provider="claude",
                                **_env_sessao(meta.get("subagent_model"), bool(meta.get("jev")), nome=name,
                                              claude_settings=meta.get("claude_settings"))):
            headless_sessions.restaurar(meta)
            raise ValueError("falha ao criar o terminal; a sessao segue sem terminal")
        self._seed(name, jsonl)
        return SessionInfo(name=name, cwd=meta["cwd"], jsonl=jsonl, tracked=True,
                           provider="claude", engine=meta.get("engine"),
                           engine_account=meta.get("engine_account"),
                           conta=meta.get("engine_credential_id"))

    def wait_for_claude(self, name: str, meta: dict, timeout: float = 20.0) -> None:
        from app.terminal_input import _wait_input_ready
        service_tier = _claude_service_tier(meta.get("engine"), meta.get("model"), meta.get("service_tier"))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            procinfo._invalidar_children_map()
            pane = self._pane_of(name)
            provider, agent = agente_do_pane((pane or {}).get("pid"))
            if provider == "claude" and agent and procinfo.pid_vivo(agent):
                if _session_id_from_cmdline(_cmdline(agent)) == meta["session_id"]:
                    expected_config = Path(meta.get("config_dir") or Path.home() / ".claude").resolve()
                    actual_config = Path(_config_dir_of(agent) or Path.home() / ".claude").resolve()
                    if (_engine_of(agent) != meta.get("engine") or actual_config != expected_config
                            or procinfo._env_var_of(agent, "CP_ENGINE_SERVICE_TIER") != service_tier
                            or procinfo._env_var_of(agent, "CP_ENGINE_ACCOUNT") != meta.get("engine_account")
                            or (meta.get("engine_account") and procinfo._env_var_of(agent, "CP_ENGINE_CREDENTIAL_ID")
                                != meta.get("engine_credential_id"))
                            or (meta.get("engine_account_base_url") and procinfo._env_var_of(agent, "CP_ENGINE_ACCOUNT_BASE_URL")
                                != meta["engine_account_base_url"])):
                        raise ValueError("o Claude reabriu com uma identidade diferente da escolhida")
                    if not _wait_input_ready(name, timeout=max(0.0, deadline - time.monotonic())):
                        raise ValueError("o terminal não ficou pronto; confira login, confiança ou pergunta pendente")
                    if not procinfo.pid_vivo(agent):
                        raise ValueError("o Claude saiu durante a reabertura")
                    return
            if not tmux.has_session(name):
                raise ValueError("o terminal encerrou antes de retomar o Claude")
            time.sleep(0.25)
        raise ValueError("o Claude não retomou a conversa e a identidade escolhidas no prazo")

    @staticmethod
    def _comando_terminal(meta: dict, *, resume: bool, engine_models: list[dict] | None = None) -> str:
        sid = meta["session_id"]
        uuid.UUID(sid)
        # Modo de permissão vai junto: sem a flag a TUI nasce no defaultMode da conta.
        model = meta.get("model")
        if model and not meta.get("engine"):
            from app import default_model
            # Na conta Anthropic, id de motor herdado do processo antigo derruba cada turno.
            if not default_model.anthropic(model):
                model_args.validar("claude", model, None)  # valor malformado continua recusado
                _log.warning("modelo %r não é da Anthropic; sessão relançada no padrão da conta", model)
                model = None
        if meta.get("engine_account"):
            from app import cliproxy, engines
            binding = cliproxy.engine_env(meta["engine"], model, meta.get("context_window"), meta["engine_account"],
                                          home=(meta.get("engine_credential_id") or "").removeprefix("codex:"),
                                          expected_base=meta.get("engine_account_base_url"), models=engine_models)
            model = binding["ANTHROPIC_MODEL"]
        service_tier = _claude_service_tier(meta.get("engine"), model, meta.get("service_tier"))
        from app.adapters.claude import terminal_command
        argv = terminal_command(meta.get("cwd"), sid, model, meta.get("effort"),
                                meta.get("permission_mode"), meta.get("claude_settings"), resume=resume)
        cmd = tmux.join_cmd(argv)
        if meta.get("engine"):
            from app import engines
            if meta["engine"] not in engines.listar():
                raise ValueError(f"motor '{meta['engine']}' nao existe")
            _exigir_cp_engine()
            pre = ["hangar-engine", "--exec", meta["engine"]]
            if meta.get("engine_account"):
                pre += ["--account", meta["engine_account"], "--account-home",
                        binding["CP_ENGINE_CREDENTIAL_ID"].removeprefix("codex:"),
                        "--account-base-url", binding["CP_ENGINE_ACCOUNT_BASE_URL"]]
            if model:
                pre += ["--model", model]
                if meta.get("context_window"):
                    pre += ["--context", str(meta["context_window"])]
            if service_tier is not None:
                # O lançador e o pré-voo leem as mesmas fontes antes de encerrar a origem.
                env = {**os.environ, "CLAUDE_CONFIG_DIR": meta.get("config_dir") or str(Path.home() / ".claude")}
                engines.service_tier_settings(argv, env, service_tier, cwd=meta["cwd"])
                pre += ["--service-tier", service_tier]
            cmd = tmux.join_cmd(pre + ["--"]) + " " + cmd
        if meta.get("read_only"):
            from app.orq_readonly import prepare
            # Refeita para a conta em que reabre; sem bwrap, recusa em vez de abrir sem a proteção.
            prefix = prepare(meta["cwd"], runtime_dirs=(meta.get("config_dir") or "",))
            cmd = tmux.join_cmd([*prefix, "/bin/sh", "-c", cmd])
        return cmd

    def para_headless(self, name: str, permission_mode: str | None, *, transfer_meta: dict | None = None,
                      for_account_move: bool = False, target_config_dir: str | None = None) -> dict:
        """Pane tmux vira sessão sem terminal. Devolve o sidecar gravado; subir o processo é da
        API (async). `permission_mode` é o que o rodapé mostra agora (lido por quem chama).
        `target_config_dir` é a conta onde o terminal reabre: só com ela uma sessão read-only
        estaciona, porque o `para_terminal` refaz a proteção nessa conta."""
        if codex_sessions.exists(name) or headless_sessions.exists(name):
            raise ValueError("so sessao Claude no terminal pode ficar sem terminal")
        pane = self._pane_of(name)
        if pane is None:
            raise ValueError("sessao nao encontrada")
        read_only = self._refuse_non_claude_resume(pane, read_only_ok=target_config_dir is not None)
        cwd, pid = pane["cwd"], pane.get("pid")
        jsonl, tracked = self.resolve_tracked(name, cwd)
        if not jsonl or not tracked:
            raise ValueError("sessao sem id: nao sei qual conversa continuar")
        sid = Path(jsonl).stem
        uuid.UUID(sid)
        # Tudo lido do processo vivo ANTES do kill: o /proc (ou o psutil) some com ele.
        ag = _pid_do_agente(pid)
        claude_settings = session_customizations.from_environment(
            procinfo._env_var_of(ag, session_customizations.SESSION_SETTINGS_ENV) if ag else None)
        if claude_settings is None:
            claude_settings = session_customizations.stored_settings(sid)
        cdir = _config_dir_of(ag) if ag else None
        origin_prefix: list[str] = []
        if read_only:
            from app.orq_readonly import prepare
            # Antes do kill: sem bwrap utilizável na conta destino, a sessão não pode morrer. O
            # prefixo da origem é do pane de volta, se o sidecar não gravar.
            prepare(cwd, runtime_dirs=(target_config_dir,))
            origin_prefix = prepare(cwd, runtime_dirs=(str(cdir) if cdir else "",))
        motor = _engine_of(ag) if ag else None
        engine_account = procinfo._env_var_of(ag, "CP_ENGINE_ACCOUNT") if ag and motor else None
        engine_credential_id = procinfo._env_var_of(ag, "CP_ENGINE_CREDENTIAL_ID") if engine_account else None
        engine_account_base_url = procinfo._env_var_of(ag, "CP_ENGINE_ACCOUNT_BASE_URL") if engine_account else None
        service_tier = procinfo._env_var_of(ag, "CP_ENGINE_SERVICE_TIER") if ag and motor else None
        modelo, esforco = procinfo._model_of(ag) if ag else (None, None)
        janela = procinfo._env_var_of(ag, "CLAUDE_CODE_MAX_CONTEXT_TOKENS") if ag else None
        # Com motor a variável é do motor (engines.env_de); sem motor veio do `-e` da criação.
        subagente = procinfo._env_var_of(ag, "CLAUDE_CODE_SUBAGENT_MODEL") if ag and not motor else None
        jev = _jev_do_processo(ag)
        if motor:
            from app import engines
            if motor not in engines.listar() and not for_account_move:
                if engine_account:
                    raise ValueError("motor da conta ChatGPT fixa indisponível")
                # Mesmo fallback do resume(): escolha de motor apagado não vale na conta Anthropic.
                motor = modelo = esforco = janela = service_tier = None
            else:
                # /model não altera argv; retomar o modelo do boot desfaria a escolha em uso.
                live_model, live_effort = _escolhas_status(sid)
                modelo = live_model or modelo
                esforco = _esforco_de_abertura(live_effort) or esforco
        else:
            # O cmdline só sabe o modelo do boot; `/model` na TUI, ou sessão aberta sem `--model`,
            # só aparecem no que a statusline recebeu. Com `[1m]` no id, a janela vai junto.
            vivo_m, vivo_e = _escolhas_status(sid)
            modelo, esforco = vivo_m or modelo, _esforco_de_abertura(vivo_e) or esforco
        if engine_account and not for_account_move:
            from app import cliproxy
            binding = cliproxy.engine_env(motor, modelo, None, engine_account,
                                          home=(engine_credential_id or "").removeprefix("codex:"),
                                          expected_base=engine_account_base_url)
            modelo = binding["ANTHROPIC_MODEL"]
        if not for_account_move:
            service_tier = _claude_service_tier(motor, modelo, service_tier)
        elif service_tier not in (None, "default", "priority"):
            raise ValueError("service_tier: use default ou priority")
        model_args.validar("claude", modelo, esforco, permission_mode)
        if transfer_meta and (sid != transfer_meta["session_id"] or cwd != transfer_meta["cwd"]
                              or pid != transfer_meta.get("pane_pid")):
            raise ValueError("session_transfer_source_changed")
        filhos = _descendant_pids(pid) if pid else []
        headless_sessions.marcar_troca(name)
        if not tmux.kill_session(name):
            raise KillFailed(name)
        # Dois `claude` no mesmo .jsonl é conversa corrompida: o do pane sai antes do novo subir.
        _esperar_saida(filhos)
        self._forget(name)
        try:
            from app import runtime_coordinator
            coordinator = runtime_coordinator.current()
            owner_key = coordinator.slot(name).binding.key if coordinator is not None and coordinator.managed_queue(name) else None
            meta = headless_sessions.save(name, cwd, sid, config_dir=str(cdir) if cdir else None,
                                          key=owner_key,
                                          engine=motor, model=modelo, effort=esforco,
                                          engine_account=engine_account,
                                          engine_credential_id=engine_credential_id,
                                          engine_account_base_url=engine_account_base_url,
                                          service_tier=service_tier,
                                          context_window=int(janela) if janela and janela.isdigit() else None,
                                          permission_mode=permission_mode, subagent_model=subagente,
                                          claude_settings=claude_settings, jev=jev, read_only=read_only,
                                          **({"key": transfer_meta["key"], "transfer_id": transfer_meta["transfer_id"]}
                                             if transfer_meta else {}),
                                          previous_non_plan=((modo_permissao.session_non_plan_mode(jsonl)
                                              or modo_permissao.modo_da_conta(str(cdir) if cdir else None))
                                              if permission_mode == "plan" else None))
        except OSError as e:
            meta = {"name": name, "cwd": cwd, "session_id": sid, "config_dir": str(cdir) if cdir else None,
                    "engine": motor, "model": modelo, "effort": esforco, "permission_mode": permission_mode,
                    "engine_account": engine_account, "engine_credential_id": engine_credential_id,
                    "engine_account_base_url": engine_account_base_url, "service_tier": service_tier,
                    "claude_settings": claude_settings}
            try:
                cmd = self._comando_terminal(meta, resume=Path(jsonl).exists())
                if read_only:
                    cmd = tmux.join_cmd([*origin_prefix, "/bin/sh", "-c", cmd])
                reaberto = tmux.new_session(name, cwd, cmd, meta["config_dir"], provider="claude",
                                            **_env_sessao(subagente, jev, nome=name, claude_settings=claude_settings))
                if reaberto:
                    # `new-session` volta 0 mesmo quando o comando morre ao nascer: conta o pane vivo após o boot.
                    from app.terminal_input import _wait_input_ready
                    _wait_input_ready(name, timeout=10.0)
                    reaberto = tmux.has_session(name)
            except Exception:
                _log.exception("troca para sem terminal: o pane de volta de %s não montou", name)
                reaberto = False
            if not reaberto:
                _log.error("troca para sem terminal: sidecar e pane falharam, sessao %s ficou sem nada", name)
                # O pane já morreu: dizer só "não troquei" esconderia que a sessão acabou.
                raise OSError(f"{e}; o terminal também não reabriu e a sessão foi encerrada") from e
            raise
        self._seed(name, jsonl)
        return meta

    def transfer_origin(self, info: SessionInfo) -> tuple[dict, dict]:
        from app.conversation_transfer import prepare_origin, _processes, TransferError
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        meta = headless_sessions.load(info.name)
        if meta:
            if meta.get("read_only"):
                # Estacionada à espera do terminal protegido: o destino a rodaria sem a proteção.
                raise TransferError("session_transfer_read_only")
            original = dict(meta)
            model, effort = get_adapter(CLAUDE_HEADLESS).escolhas(info.name)
            meta = {**meta, "model": model or meta.get("model"), "effort": effort or meta.get("effort")}
            root = (meta.get("cano") or {}).get("pid")
        else:
            pane = self._pane_of(info.name)
            if not pane:
                raise TransferError("session_transfer_source_changed")
            self._refuse_non_claude_resume(pane)
            root = pane.get("pid")
            agent = _pid_do_agente(root) if root else None
            if agent and procinfo._engine_of(agent):
                raise TransferError("session_transfer_source_changed")
            if agent and procinfo._env_var_of(agent, "HANGAR_ORQ_READ_ONLY") == "1":
                raise TransferError("session_transfer_read_only")
            cdir = str(_config_dir_of(agent)) if agent and _config_dir_of(agent) else (info.conta or "").removeprefix("claude:")
            mode = modo_permissao.ler_modo(info.name)
            model, effort = _escolhas_status(Path(info.jsonl).stem)
            meta = {"name": info.name, "cwd": info.cwd, "session_id": Path(info.jsonl).stem,
                    "config_dir": cdir or str(Path.home() / ".claude"), "provider": "claude", "headless": False,
                    "model": model, "effort": _esforco_de_abertura(effort), "permission_mode": mode,
                    "previous_non_plan": modo_permissao.session_non_plan_mode(info.jsonl)
                                         if mode == "plan" else None,
                    "pane_pid": root, "pane_id": pane.get("pane_id"),
                    "subagent_model": procinfo._env_var_of(agent, "CLAUDE_CODE_SUBAGENT_MODEL") if agent else None,
                    "jev": _jev_do_processo(agent)}
            claude_settings = session_customizations.from_environment(
                procinfo._env_var_of(agent, session_customizations.SESSION_SETTINGS_ENV) if agent else None)
            if claude_settings is not None:
                meta["claude_settings"] = claude_settings
            original = dict(meta)
        from app.conversation_transfer import _META_FIELDS
        public = prepare_origin({key: value for key, value in meta.items() if key in _META_FIELDS})
        public.update(jsonl=info.jsonl, headless=bool(info.headless))
        return public, {"original": original, "processes": _processes(root)}

    async def stop_transfer_source(self, record, private: dict) -> None:
        from app.conversation_transfer import _processes_stopped, _process_identity, TransferError
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        processes = private["processes"]
        for pid, identity in processes.items():
            if procinfo.pid_vivo(int(pid)) and _process_identity(int(pid)) != identity:
                raise TransferError("session_transfer_process_identity_changed")
        meta = record.origin_meta
        if meta["headless"]:
            headless_sessions.update(record.name, transfer_id=record.id)
            hl = get_adapter(CLAUDE_HEADLESS)
            await hl.parar(record.name)
            if not _processes_stopped(processes):
                for pid, identity in processes.items():
                    if procinfo.pid_vivo(int(pid)) and _process_identity(int(pid)) != identity:
                        raise TransferError("session_transfer_process_identity_changed")
                await asyncio.to_thread(hl.close_sync, record.name, private["original"])
        else:
            await asyncio.to_thread(self.para_headless, record.name, meta["permission_mode"],
                                    transfer_meta={**meta, "transfer_id": record.id})
        await asyncio.to_thread(_esperar_saida, [int(pid) for pid in processes])
        if not await asyncio.to_thread(_processes_stopped, processes):
            raise TransferError("session_transfer_source_not_stopped")

    async def publish_transfer(self, record) -> None:
        from app.conversation_transfer import verify_source, TransferError
        from app.conversation_history import verify_boundary
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        import hashlib
        await asyncio.to_thread(verify_source, record.source)
        if await asyncio.to_thread(lambda: hashlib.sha256(Path(record.origin_meta["jsonl"]).read_bytes()).hexdigest()) != record.source.digest:
            raise TransferError("session_transfer_source_changed")
        from dataclasses import replace
        from app.conversation_transfer import TransferPhase
        await asyncio.to_thread(verify_boundary, replace(record, phase=TransferPhase.COMPLETE), record.boundary.rollout_path)
        await get_adapter("codex").publish_imported(record)
        original = headless_sessions.load(record.name)
        if original:
            if original.get("session_id") != record.origin_meta["session_id"] or original.get("key") != record.origin_meta["key"]:
                raise TransferError("session_transfer_source_changed")
            headless_sessions.delete(record.name)
        get_adapter(CLAUDE_HEADLESS)._sessions.pop(record.name, None)
        # Fora do laço: com o Rust dono, esquecer e semear são chamadas à ponte.
        await asyncio.to_thread(self._forget, record.name)
        await asyncio.to_thread(self._seed, record.name, record.boundary.rollout_path, True)

    async def restore_transfer_source(self, record, private: dict) -> None:
        from app.conversation_transfer import _processes_stopped, TransferError
        from app.adapters import get_adapter, CLAUDE_HEADLESS
        await get_adapter("codex").abort_imported(record)
        meta = record.origin_meta
        import hashlib
        if record.source and await asyncio.to_thread(
                lambda: hashlib.sha256(Path(meta["jsonl"]).read_bytes()).hexdigest()) != record.source.digest:
            raise TransferError("session_transfer_source_changed")
        current = headless_sessions.load(record.name)
        if current and (current.get("session_id") != meta["session_id"]
                        or current.get("key") != meta["key"]):
            raise TransferError("session_transfer_source_changed")
        if not _processes_stopped(private["processes"]):
            # Uma parada recusada pode conservar exatamente a origem viva.
            from app.conversation_transfer import _check_source_idle
            await _check_source_idle(self, record.name, meta)
            if meta["headless"]:
                from app.conversation_transfer import _runtime_source_view
                view = _runtime_source_view(record.name)
                if view is not None:
                    if not view.get("alive") or view.get("iniciando"):
                        raise TransferError("session_transfer_source_not_stopped")
                    return
                sess = get_adapter(CLAUDE_HEADLESS)._sessions.get(record.name)
                if not sess or not sess.vivo or sess.iniciando:
                    raise TransferError("session_transfer_source_not_stopped")
            else:
                pane = await asyncio.to_thread(self._pane_of, record.name)
                if not pane or pane.get("pid") != meta.get("pane_pid"):
                    raise TransferError("session_transfer_source_changed")
                if not await asyncio.to_thread(_pid_do_agente, pane["pid"]):
                    raise TransferError("session_transfer_source_not_stopped")
            return
        restored = {**private["original"], **meta, "headless": True, "transfer_id": record.id, "cano": None}
        if meta["headless"]:
            if await asyncio.to_thread(tmux.has_session, record.name):
                raise TransferError("session_transfer_source_changed")
            old_pid = (current or {}).get("cano", {}).get("pid") if (current or {}).get("cano") else None
            from app.conversation_transfer import _process_identity
            stale_process = (old_pid and str(old_pid) in private["processes"] and
                             _process_identity(old_pid) != private["processes"][str(old_pid)])
            if not current or stale_process:
                headless_sessions.restaurar(restored)
            hl = get_adapter(CLAUDE_HEADLESS)
            hl._subidas.pop(record.name, None)
            from app import runtime_coordinator
            coordinator = runtime_coordinator.current()
            if (coordinator is not None and getattr(coordinator, "transport", None) is not None
                    and coordinator.managed_queue(record.name)):
                # Com o Rust de pé a origem volta nele: o processo sobe sem cliente Python.
                try:
                    slot = await coordinator.reopen_in_change(record.name, wait_initialized=True)
                except Exception as exc:
                    from app import diag
                    diag.registrar("runtime.transfer_restore_failed", "erro", sessao=record.name,
                                   **runtime_coordinator.failure_reason(exc))
                    raise TransferError("session_transfer_restore_failed") from exc
                view = slot.view.get("view") or {}
                if not view.get("alive") or view.get("conversation") != meta["session_id"]:
                    raise TransferError("session_transfer_restore_failed")
            else:
                sess = await hl.ensure_running(record.name, transfer_id=record.id)
                if not sess or not sess.vivo or sess.iniciando or sess.sid != meta["session_id"]:
                    raise TransferError("session_transfer_restore_failed")
        else:
            from app.conversation_transfer import _processes, _process_identity, _runtime_path, _write_json
            command = self._comando_terminal(meta, resume=True)
            env = _env_sessao(meta.get("subagent_model"), bool(meta.get("jev")), nome=record.name)["env"]
            env["CP_SESSION_KEY"] = meta["key"]
            if await asyncio.to_thread(tmux.has_session, record.name):
                pid = await asyncio.to_thread(tmux.pane_pid, record.name)
                if (pid != private.get("restore_pane_pid") or
                        _process_identity(pid) != private.get("restore_processes", {}).get(str(pid))):
                    raise TransferError("session_transfer_source_changed")
            else:
                headless_sessions.restaurar(restored)
                if not await asyncio.to_thread(tmux.new_session, record.name, meta["cwd"], command,
                                               meta.get("config_dir"), provider="claude", env=env):
                    raise TransferError("session_transfer_restore_failed")
                pid = await asyncio.to_thread(tmux.pane_pid, record.name)
                private.update(restore_pane_pid=pid, restore_processes=await asyncio.to_thread(_processes, pid))
                await asyncio.to_thread(_write_json, _runtime_path(record), private)
            deadline = time.monotonic() + 45
            loaded = False
            while time.monotonic() < deadline:
                pane = await asyncio.to_thread(self._pane_of, record.name)
                agent = await asyncio.to_thread(_pid_do_agente, (pane or {}).get("pid")) if pane else None
                if agent:
                    path = Path(meta["config_dir"]) / "sessions" / f"{agent}.json"
                    try:
                        native = json.loads(await asyncio.to_thread(path.read_text, encoding="utf-8"))
                        loaded = native.get("sessionId") == meta["session_id"] and native.get("status") == "idle"
                    except (OSError, ValueError):
                        pass
                if loaded:
                    break
                await asyncio.sleep(0.1)
            if not loaded:
                raise TransferError("session_transfer_restore_failed")
            headless_sessions.delete(record.name)
        await asyncio.to_thread(self._forget, record.name)
        await asyncio.to_thread(self._seed, record.name, meta["jsonl"], True)

    def rename(self, old: str, new: str) -> None:
        from app.conversation_transfer import require_available
        require_available(old)
        require_available(new)

        if headless_sessions.exists(old):
            from app.adapters import get_adapter, CLAUDE_HEADLESS
            new = sanitize_session_name(new)
            if not new or new == old:
                return
            if tmux.has_session(new) or codex_sessions.exists(new) or headless_sessions.exists(new):
                raise ValueError("ja existe uma sessao com esse nome")
            headless_sessions.rename(old, new)
            get_adapter(CLAUDE_HEADLESS).rename(old, new)
            self._jsonl_cache.pop(old, None)
            self._rename_rust(old, new)
            PromptQueue(old).rename(new)
            ThenLink(old).rename(new)
            _rename_pair_python(old, new)
            shortcut_terminals.rename_owner(old, new)
            from app.conversation_transfer import rename_transfer
            rename_transfer(old, new)
            return
        if codex_sessions.exists(old):
            from app.adapters import get_adapter
            codex_sessions.rename(old, new)
            get_adapter("codex").rename(old, new)
        # Cache e keyed por NOME -> ao renomear, move a entrada pro nome novo e esquece o velho. Senao
        # o nome velho apontaria pro jsonl pra sempre (reuso futuro = transcript errado) e o nome novo
        # cairia no fallback newest-by-mtime ate um sinal confiavel reaparecer.
        j = self._jsonl_cache.pop(old, None)
        if j is not None:
            self._jsonl_cache[new] = j
        if old in self._fd_locked:           # move o fd-lock junto com o cache
            self._fd_locked.discard(old)
            self._fd_locked.add(new)
        st = self._status_cache.pop(old, None)   # statusline move junto (mesma sessao, so outro nome)
        if st is not None:
            self._status_cache[new] = st
        if old in self._label_cache:
            self._label_cache[new] = self._label_cache.pop(old)
        if old in self._limit_cache:
            self._limit_cache[new] = self._limit_cache.pop(old)
        if old in self._reply_cache:
            self._reply_cache[new] = self._reply_cache.pop(old)
        self._rename_rust(old, new)
        # A fila duravel tambem e keyed por NOME -> move junto, senao a sessao renomeada perde as
        # entradas nao-drenadas e elas ficam orfas no nome velho (fantasma se reusarem `old`).
        PromptQueue(old).rename(new)
        # Vinculo 'then' (feature #12): mesmo motivo — keyed por NOME, move junto pra sessao renomeada
        # nao perder o encadeamento armado.
        ThenLink(old).rename(new)
        # Pareamento: move o próprio sidecar E re-aponta o do PAR (que referencia o nome velho) —
        # senão o par ficaria pareado com um fantasma e o unpair simétrico quebrava. Sob o lock do
        # módulo pair (rename_pair): sem ele, um unpair concorrente podia ser ressuscitado.
        _rename_pair_python(old, new)
        if apos_renomear_codex:
            apos_renomear_codex(old, new)
        # L71 da revisao final: o shell escondido e keyed por NOME (`term-<nome>`) e NAO acompanha o
        # rename sozinho -- ele ficava orfa pra sempre, invisivel no app (marcado @cp_hidden) e fora
        # do alcance do `kill()`, que so procura `term-<nome NOVO>`. Pior: a aba Shell do nome novo
        # criaria um shell NOVO e o velho seguiria vivo consumindo o nome, ate colidir com uma
        # sessao futura.
        # RENOMEIA, nao mata: `rename-session` nao mexe no cwd nem no que esta rodando no pane -- o
        # shell continua no mesmo diretorio, que e o diretorio da sessao renomeada. (O perigo de
        # "shell no diretorio errado" e outro caminho: reatar um `term-<nome>` orfa de OUTRO repo,
        # tratado no tmux.new_hidden_shell.) Matar em silencio derrubaria um `npm run dev` que
        # estivesse rodando ali, e o unico registro disso e um `_log.debug`.
        # Kill so como FALLBACK: renomear falha se `term-<novo>` ja existir (shell de uma vida
        # anterior daquele nome), e ai deixar o velho vivo traz de volta o orfa que este bloco
        # existe pra evitar.
        # A marca e o gate, como no `_kill_hidden_shell`: sem ela, um `term-<velho>` de TERCEIRO
        # seria sequestrado pelo rename. `is_hidden` mira `={nome}:` (exato), entao o rename so
        # roda quando a sessao existe de verdade -- sem risco do prefix-match do tmux pegar
        # `term-<velho>-2`.
        # Terminais de atalho seguem a conversa pelo dono gravado neles (nao pelo nome tmux).
        shortcut_terminals.rename_owner(old, new)
        from app.conversation_transfer import rename_transfer
        rename_transfer(old, new)
        alvo = f"term-{old}"
        if tmux.is_hidden(alvo) and not tmux.rename_session(alvo, f"term-{new}"):
            _log.info("rename: %r nao pode virar %r (nome ja ocupado?) — matando o shell escondido",
                      alvo, f"term-{new}")
            self._kill_hidden_shell(old)

    @staticmethod
    def _kill_hidden_shell(name: str) -> None:
        # Task 6 (achado da revisao, rodada 2): mata `term-<name>` SO se a marca confirmar que a
        # sessao e NOSSA -- consulta DIRETA ao tmux (`is_hidden`), nao inferida de `self.list()`
        # (que tambem filtra por sidecar Codex, e "sumir da lista" nao e o mesmo que "estar
        # marcada"). Sem esta checagem, um "term-<name>" de TERCEIRO (o mesmo cenario alcancavel
        # que o I1 reconheceu na rota /shell) seria derrubado JUNTO quando o agente `name` fosse
        # encerrado, com trabalho rodando e sem afordancia nenhuma pro dono perceber -- so um
        # `_log.debug`. Best-effort: falhar aqui NAO pode derrubar o kill principal, que ja
        # aconteceu antes desta chamada.
        alvo = f"term-{name}"
        if tmux.is_hidden(alvo) and not tmux.kill_session(alvo):
            _log.debug("kill: shell escondido de %r nao saiu (pode nao existir)", name)

    def kill(self, name: str) -> list[dict]:
        """Devolve os avisos de saída do grupo que o Rust não entregou (vazio no modo Python, onde a
        rota avisa depois)."""
        from app.conversation_transfer import require_available
        require_available(name)

        # Levanta KillFailed quando a sessao SOBREVIVE. Antes o retorno do tmux era descartado e a
        # limpeza duravel (cache, fila, then, pareamento) rodava do mesmo jeito: o card sumia da UI, o
        # pareamento se desfazia, a rota respondia {"ok": true} — e a sessao reaparecia na varredura
        # seguinte, sem fila e sem par, parecendo um bug sem relacao com o "encerrar" de minutos antes.
        # Pesa mais no Windows, onde o kill-session do psmux nao derruba a sessao (medido).
        if headless_sessions.exists(name):
            # Claude sem terminal: SIGTERM no processo (se vivo), sidecar fora, estado durável limpo.
            from app.adapters import get_adapter, CLAUDE_HEADLESS
            # Sidecar PRIMEIRO: sem ele, um drain que chegue no meio não sobe outro processo. O
            # meta vai junto: é nele que mora o pid do cano, que vive fora do backend.
            meta = headless_sessions.load(name)
            headless_sessions.delete(name)
            try:
                get_adapter(CLAUDE_HEADLESS).close_sync(name, meta)
            except Exception:
                headless_sessions.restaurar(meta, preserve_process=True)
                raise
            shortcut_terminals.close_all(name)
            self._forget(name)
            PromptQueue(name).clear()
            ThenLink(name).clear()
            return self._clear_pair(name)
        if codex_sessions.exists(name):
            # Sessao Codex: fecha app-server e TUI tmux, apaga o sidecar e limpa estado duravel.
            from app.adapters import get_adapter
            # O tmux PRIMEIRO. Invertido, `close_sync` matava o app-server e so entao o kill era
            # tentado: falhando ele (o caso que esta funcao existe pra pegar), a excecao dizia "nao
            # consegui encerrar" com a TUI ja sem servidor — sinal trocado, e a sessao que sobrou na
            # tela nao fala mais com ninguem. Agora falha antes de qualquer estrago.
            if not (codex_sessions.load(name) or {}).get("headless") and not tmux.kill_session(name):
                raise KillFailed(name)
            get_adapter("codex").close_sync(name)
            self._kill_hidden_shell(name)
            shortcut_terminals.close_all(name)
            codex_sessions.delete(name)
            self._forget(name)
            PromptQueue(name).clear()
            ThenLink(name).clear()
            warnings = self._clear_pair(name)
            if apos_saida_codex:
                apos_saida_codex(name)
            return warnings
        # Limpa o sidecar do AskUserQuestion ANTES de matar (precisa do processo vivo pra resolver o
        # jsonl), best-effort: cleanup nunca bloqueia/quebra o kill. Senao um stale reabriria o stepper
        # numa sessao futura de mesmo nome.
        try:
            jsonl = next((s.jsonl for s in self.list() if s.name == name), None)
            if jsonl:
                clear_pending_askq(jsonl)
        except Exception:
            pass
        if not tmux.kill_session(name):
            raise KillFailed(name)
        self._kill_hidden_shell(name)
        shortcut_terminals.close_all(name)
        self._forget(name)  # cache invalido: nome pode ser reusado por outra sessao depois
        # Sessao morta nao deixa fila pra tras: senao acumula orfaos e uma futura sessao de mesmo
        # nome herdaria essas entradas como bubble-fantasma (mesmo motivo do clear no create()).
        PromptQueue(name).clear()
        ThenLink(name).clear()  # mesmo motivo, pro vinculo 'then' (feature #12)
        return self._clear_pair(name)

    @staticmethod
    def _clear_pair(name: str) -> list[dict]:
        # Sessão morta SAI do grupo (leave: sob lock, atualiza os demais membros): sem isto os
        # companheiros apontariam pra um fantasma (badge preso). Best-effort, nunca bloqueia o
        # kill nem a criação — mas LOGA: engolir calado deixava o badge-fantasma indiagnosticável.
        from app import groups_bridge
        if groups_bridge.rust_owns_groups():
            # O Rust já avisa as outras máquinas e o par externo; falha aqui a varredura dele resolve.
            try:
                out = groups_bridge.call("group.leave", name=name)
            except groups_bridge.GroupsBridgeError as e:
                _log.warning("_clear_pair(%s): o Rust não tirou a sessão do grupo: %s", name, e.code)
                # A varredura resolver depois não é ter saído agora: o aviso vai na resposta.
                return [{"sessao": name, "erro": erro("erro_grupo_indisponivel",
                                                      "os grupos estão indisponíveis agora", detalhe=e.code)}]
            warnings = out.get("warnings")
            return warnings if isinstance(warnings, list) else []
        try:
            pair_leave(name)
        except Exception as e:
            # Sem "kill(...)" no texto: o create() também chama isto (nome reusado de sessão morta
            # fora do kill), e a falha aparecia no log como se fosse de um encerramento.
            _log.warning("_clear_pair(%s): falha ao sair do grupo de pareamento: %r", name, e)
        return []

    @staticmethod
    def _leave_old_group(name: str) -> None:
        """Nome reusado no modo Rust: o grupo da sessão antiga sai antes de qualquer efeito da
        criação. Sem limpeza e com o sidecar ainda lá, a criação é recusada."""
        from app import groups_bridge
        if not groups_bridge.rust_owns_groups():
            return
        if tmux.has_session(name) or codex_sessions.exists(name) or headless_sessions.exists(name):
            return   # nome em uso: o ramo da criação recusa, e o grupo é de quem está viva
        try:
            out = groups_bridge.call("group.leave", name=name)
            _log_leave_warnings(name, out.get("warnings"))
        except groups_bridge.GroupsBridgeError as e:
            if PairLink(name).path.exists():
                _log.warning("criação de %s recusada: grupo antigo não desfeito (%s)", name, e.code)
                raise GroupCleanupFailed(name) from e
            _log.warning("criação de %s: ponte de grupos falhou sem grupo a desfazer (%s)", name, e.code)

    def sweep_pairs(self, list_fn: Callable[[], list[SessionInfo]], agora: float | None = None) -> None:
        """Lista que falha levanta antes de varrer: vazia por erro dissolveria grupos vivos. Sem
        nenhum pareado, nem pergunta a lista."""
        vivos = {i.name for i in list_fn()} if pair.referenciados_locais() else set()
        self._varrer_pares_mortos(vivos, agora)

    def _varrer_pares_mortos(self, vivos: set[str], agora: float | None = None) -> None:
        """Membro de grupo cuja sessão morreu FORA do app (Ctrl-C, crash, reboot): ninguém chamou
        leave, o sidecar apontava pra um fantasma pra sempre. Morto = ausente da lista viva numa
        varredura anterior E há pelo menos _PAIR_AUSENCIA_MIN_S — kill() e rename() deixam o nome
        ausente de propósito por um instante, e só o tempo separa isso de morte. Roda no laço
        `pair_sweep_loop`, fora da descoberta."""
        try:
            sozinhos = pair.dissolve_lone_orq()
        except Exception as e:
            _log.warning("varredura de pares: grupo orq de um membro não dissolvido: %r", e)
        else:
            if sozinhos:
                _log.info("varredura de pares: grupo orq sem execução viva dissolvido (%s)", sozinhos)
        if not vivos:
            return   # tmux fora = lista vazia; varrer aqui dissolveria todos os grupos
        agora = time.monotonic() if agora is None else agora
        # referenciados_locais() devolve stem SANITIZADO (_sanitize do pqueue); vivos é nome CRU do
        # tmux — sem subtrair a versão sanitizada de vivos, sessão com espaço/acento no nome nunca
        # sai de candidatos e a varredura dissolve um grupo vivo (achado do review final).
        candidatos = pair.referenciados_locais() - vivos - {_sanitize(v) for v in vivos}
        cls = type(self)
        for n in [x for x in dict(cls._pair_ausencias) if x not in candidatos]:
            cls._pair_ausencias.pop(n, None)  # varredura concorrente (2 registries) pode já ter tirado
        for n in candidatos:
            primeira = cls._pair_ausencias.setdefault(n, agora)
            if agora - primeira < cls._PAIR_AUSENCIA_MIN_S:
                continue
            cls._pair_ausencias.pop(n, None)  # idem: 2 threads podem passar o portão de tempo juntas
            try:
                ex = pair_leave(n)
            except Exception as e:
                _log.warning("varredura de pares: '%s' morto fora do app, leave falhou: %r", n, e)
                continue
            _log.info("varredura de pares: '%s' morreu fora do app; saiu do grupo (%s)", n, ex)
            _encerrar_pares_externos(n)
            if any("::" in p for p in ex):
                _log.warning("varredura de pares: '%s' tinha par remoto; sidecar de lá fica órfão", n)

    # ── Resume de sessao "sem id" ────────────────────────────────────────────────
    # Uma sessao aberta com `claude` cru (sem --session-id) JA tem um transcript <uuid>.jsonl; so nao da
    # pra ligar o pane a ele com seguranca (o uuid nao esta no cmdline). Relançar o pane com
    # `claude --resume <uuid>` poe o uuid no cmdline -> resolve() volta a rastrear (tracked=True) e o chat
    # abre CONTINUANDO a mesma conversa. Reusa kill+new_session (trata cores/config-dir corretamente).

    def _pane_of(self, name: str) -> Optional[dict]:
        return next((p for p in tmux.list_panes_active() if p["name"] == name), None)

    def _first_user_text(self, jsonl: str, max_lines: int = 60) -> str:
        # Preview do candidato = 1a msg de usuario da conversa (identifica "qual conversa e essa"). Le so
        # as primeiras linhas; import local pra evitar ciclo (transcript -> models -> ...).
        from app.transcript import parse_line
        try:
            with open(jsonl, encoding="utf-8", errors="replace") as fh:
                for _, line in zip(range(max_lines), fh):
                    for ev in parse_line(line):
                        if ev.kind == "user_msg" and ev.text:
                            return ev.text[:100]
        except OSError:
            pass
        return ""

    @staticmethod
    def _refuse_non_claude_resume(pane: dict, *, read_only_ok: bool = False) -> bool:
        """Devolve se o pane roda sob a proteção read-only; só quem a refaz passa `read_only_ok`."""
        read_only = bool(pane.get("pid")) and any(
                "HANGAR_ORQ_READ_ONLY" in _cmdline(pid)
                or procinfo._env_var_of(pid, "HANGAR_ORQ_READ_ONLY") == "1"
                for pid in _descendant_pids(pane["pid"]))
        if read_only and not read_only_ok:
            raise ValueError("sessão read-only: recrie com --read-only; retomar aqui removeria a proteção")
        # O resume e Claude-only de ponta a ponta: os candidatos saem de ~/.claude/projects e o
        # relance e `claude --resume <uuid>` DEPOIS de matar o pane. Numa sessao Pi sem transcript
        # (que agora aparece como "sem id" e por isso ganha o botao de retomar), isso ofereceria
        # conversas do CLAUDE daquele cwd e, se o usuario escolhesse uma, mataria a sessao Pi viva
        # pra subir um claude no lugar dela. Recusa com uma frase que diz o que FAZER.
        # ponytail: recusar e o piso. Retomar de verdade exige varrer ~/.pi/agent/sessions e
        # relancar com `pi --session <id>` exportando CP_PI_SESSION — o upgrade, quando alguem
        # topar com isto de verdade.
        prov = provider_of_pane(pane.get("pid"))
        if prov != "claude":
            raise ValueError(
                f"retomar so vale pra sessao Claude (esta e {prov}); "
                f"feche o pane e abra de novo pelo wrapper `{prov}`")
        if read_only_ok and not read_only and sys.platform == "linux":
            # Quem refaz a proteção reabre sem ela quando a leitura diz "não": leitura que falhou
            # não pode valer como "não".
            agent = _pid_do_agente(pane.get("pid"))
            if agent is None or not procinfo._environ_legivel(agent):
                raise ValueError("não consegui confirmar se a sessão roda protegida (read-only); nada foi encerrado")
        return read_only

    def resume_candidates(self, name: str) -> tuple[str, bool, list[dict]]:
        # (cwd, ambiguo, candidatos). ambiguo = ha OUTRA sessao tmux no mesmo cwd -> o "mais recente por
        # mtime" pode ser de outra sessao (a UI pede confirmacao). candidatos = ate 6 jsonls recentes do
        # cwd, cada um com preview + se ja esta em uso por outra sessao viva.
        pane = self._pane_of(name)
        if pane is None:
            raise ValueError("sessao nao encontrada")
        self._refuse_non_claude_resume(pane)
        cwd = pane["cwd"]
        ag = _pid_do_agente(pane.get("pid"))
        cdir = _config_dir_of(ag) if ag else None
        proj = ((cdir / "projects") if cdir else self.projects_dir) / sanitize_cwd(cwd)
        files = sorted(proj.glob("*.jsonl"),
                       key=lambda f: (f.stat().st_mtime if f.exists() else 0.0), reverse=True)[:6] \
            if proj.is_dir() else []
        taken = {os.path.realpath(s.jsonl) for s in self.list() if s.jsonl and s.name != name}
        cands = [{
            "session_id": f.stem,
            "mtime": _jsonl_mtime(str(f)),
            "preview": self._first_user_text(str(f)),
            "in_use": os.path.realpath(str(f)) in taken,
        } for f in files]
        return cwd, self._cwd_has_siblings(cwd), cands

    def resume(self, name: str, session_id: str) -> SessionInfo:
        # Relança o pane com `claude --resume <session_id>`, continuando a conversa. Valida o uuid (vai
        # DIRETO pro comando do shell -> barra injecao) e exige o .jsonl existir (nao resume fantasma).
        try:
            uuid.UUID(session_id)
        except (ValueError, AttributeError, TypeError):
            raise ValueError("session_id invalido")
        pane = self._pane_of(name)
        if pane is None:
            raise ValueError("sessao nao encontrada")
        # Tambem AQUI, e nao so no resume_candidates: com session_id vindo do corpo o endpoint pula
        # a listagem de candidatos e cai direto no relance (que mata o pane).
        self._refuse_non_claude_resume(pane)
        cwd = pane["cwd"]
        # Aberta no terminal, o pid do pane é o shell: conta, motor e modelo moram no `claude` filho.
        ag = _pid_do_agente(pane.get("pid"))
        cdir = _config_dir_of(ag) if ag else None
        # Motor da sessão que está morrendo. Sem reaplicar, uma sessão Kimi ressuscita na conta
        # Anthropic continuando um transcript de Kimi — calado. Tem que ler ANTES do kill_session: o
        # /proc do pane some com ele.
        motor = _engine_of(ag) if ag else None
        engine_account = procinfo._env_var_of(ag, "CP_ENGINE_ACCOUNT") if ag and motor else None
        engine_credential_id = procinfo._env_var_of(ag, "CP_ENGINE_CREDENTIAL_ID") if engine_account else None
        engine_account_base_url = procinfo._env_var_of(ag, "CP_ENGINE_ACCOUNT_BASE_URL") if engine_account else None
        service_tier = procinfo._env_var_of(ag, "CP_ENGINE_SERVICE_TIER") if ag and motor else None
        motor_sumiu = False
        if motor:
            from app import engines
            if motor not in engines.listar():
                if engine_account:
                    raise ValueError("motor da conta ChatGPT fixa indisponível")
                # Motor apagado no app depois de a sessão nascer: melhor voltar na conta Anthropic (o
                # badge mostra isso) do que recusar o resume e deixar a sessão inacessível. Nesse
                # fallback a escolha lida abaixo é DO MOTOR — reaplicá-la na conta Anthropic criaria
                # uma sessão inviável (id que ela não conhece). Descartar modelo, esforço e janela:
                # resume pelado, como antes desta branch.
                motor_sumiu = True
                motor = None
        # Modelo/esforço com que a sessão SUBIU, lidos do cmdline do processo que está morrendo.
        # Sem reaplicar, `claude --resume <sid>` pelado volta pro modelo do motor — a escolha some
        # sem aviso. Mesma regra do motor: ler ANTES do kill_session, o /proc do pane some com ele.
        modelo, esforco = procinfo._model_of(ag) if ag else (None, None)
        # A janela mora no MESMO /proc/<pid>/environ — lê-la junto de motor/modelo, nunca depois
        # do kill (B2 da revisão final: o kill derruba o processo e a leitura pós-kill devolve
        # nada, e a sessão ressuscitava sem --context, calado).
        janela = procinfo._env_var_of(ag, "CLAUDE_CODE_MAX_CONTEXT_TOKENS") if ag else None
        if engine_account:
            from app import cliproxy
            binding = cliproxy.engine_env(motor, modelo, None, engine_account,
                                          home=(engine_credential_id or "").removeprefix("codex:"),
                                          expected_base=engine_account_base_url)
            modelo = binding["ANTHROPIC_MODEL"]
        if motor_sumiu:
            modelo = esforco = janela = service_tier = None
        if modelo and not motor:
            from app import default_model
            if not default_model.anthropic(modelo):
                model_args.validar("claude", modelo, None)  # malformado: recusa antes do kill
                _log.warning("resume %s: modelo %r não é da Anthropic; volta no padrão da conta", name, modelo)
                modelo = None
        service_tier = _claude_service_tier(motor, modelo, service_tier)
        # Sem motor, a variável veio do `-e` da criação e sumiria no relançamento; com motor, é dele.
        subagente = (procinfo._env_var_of(ag, "CLAUDE_CODE_SUBAGENT_MODEL")
                     if ag and not motor and not motor_sumiu else None)
        # Junto do resto que sai do /proc: depois do `kill_session` lá embaixo não há mais processo
        # de onde ler, e a sessão ressuscitada nasceria com o Jev desligado sem ninguém pedir.
        jev = _jev_do_processo(ag)
        proj = ((cdir / "projects") if cdir else self.projects_dir) / sanitize_cwd(cwd)
        jsonl = proj / f"{session_id}.jsonl"
        if not jsonl.exists():
            raise ValueError("transcript nao encontrado")
        # COMANDO INTEIRO ANTES DO KILL. `args_de` valida e estoura ValueError; montar depois de
        # matar o pane trocava "resume falhou" por "sessao destruida e nao relancada" — e o gatilho
        # e real: o settings.json das contas traz `"model": "opus[1m]"`, que o proprio Claude Code
        # anexa ao nome. Nada aqui toca o tmux.
        # "claude" literal: esta funcao ja recusa provider nao-Claude acima
        # (_refuse_non_claude_resume), e nao ha variavel `provider` neste escopo.
        current_jsonl, current_tracked = self.resolve_tracked(name, cwd, pid=pane.get("pid"))
        current_sid = Path(current_jsonl).stem if current_jsonl and current_tracked else None
        process_settings = session_customizations.from_environment(
            procinfo._env_var_of(ag, session_customizations.SESSION_SETTINGS_ENV) if ag else None)
        claude_settings = session_customizations.resume_settings(session_id, current_sid, process_settings)
        from app.adapters.claude import terminal_command
        argv = terminal_command(cwd, session_id, modelo, esforco,
                                claude_settings=claude_settings, resume=True)
        cmd = tmux.join_cmd(argv)
        if motor:
            # Prefixo remontado JUNTO com a escolha: preservar so a flag deixaria a sessao
            # ressuscitada com a flag num modelo e o AMBIENTE noutro (as cinco chaves ANTHROPIC_*,
            # o SUBAGENT_MODEL e a janela voltariam pro modelo do motor).
            # Antes do kill, junto com o resto da montagem do comando — o comentario acima explica
            # por que NADA aqui pode tocar o tmux antes de o comando inteiro estar pronto: recusar
            # depois do kill trocaria "resume recusado" por "sessao destruida e nao relancada".
            _exigir_cp_engine()
            pre = ["hangar-engine", "--exec", motor]
            if engine_account:
                pre += ["--account", engine_account, "--account-home",
                        binding["CP_ENGINE_CREDENTIAL_ID"].removeprefix("codex:"),
                        "--account-base-url", binding["CP_ENGINE_ACCOUNT_BASE_URL"]]
            if modelo:
                pre += ["--model", modelo]
                if janela:
                    pre += ["--context", janela]
            if service_tier is not None:
                env = {**os.environ, "CLAUDE_CONFIG_DIR": str(cdir or Path.home() / ".claude")}
                engines.service_tier_settings(argv, env, service_tier, cwd=cwd)
                pre += ["--service-tier", service_tier]
            cmd = tmux.join_cmd(pre + ["--"]) + " " + cmd
        tmux.kill_session(name)
        self._forget(name)
        env_pane = _env_sessao(subagente, jev, nome=name, claude_settings=claude_settings)
        if not tmux.new_session(name, cwd, cmd, str(cdir) if cdir else None, **env_pane):
            raise ValueError("falha ao relançar a sessao")
        if service_tier is not None:
            self.wait_for_claude(name, {
                "session_id": session_id, "config_dir": str(cdir) if cdir else None,
                "engine": motor, "model": modelo, "service_tier": service_tier,
                "engine_account": engine_account, "engine_credential_id": engine_credential_id,
                "engine_account_base_url": engine_account_base_url,
            })
        # Fixa o transcript resumido no cache: resolve() ja o devolveria (o --resume esta no cmdline),
        # mas semear evita a janela onde o pane ainda esta subindo e cairia no fallback por mtime.
        self._jsonl_cache[name] = str(jsonl)
        return SessionInfo(name=name, cwd=cwd, jsonl=str(jsonl), tracked=True, engine=motor,
                           engine_account=engine_account, conta=engine_credential_id)


from app.runtime_adapter import registry_method as _runtime_registry_method
for _method_name in ("kill", "rename", "para_terminal", "para_headless"):
    setattr(SessionRegistry, _method_name, _runtime_registry_method(getattr(SessionRegistry, _method_name)))
