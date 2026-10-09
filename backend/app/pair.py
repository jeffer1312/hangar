"""Pareamento de sessões (feature "trabalhando juntas"): GRUPO de N sessões que colaboram em repos
complementares (ex: front + back + POS). Mesmo padrão de sidecar do ThenLink (app/chain.py): dir
irmão ".hangar-pair", um JSON pequeno por MEMBRO, keyed pelo NOME:

    {"peers": ["outra", "mais-uma"], "task": "...", "gid": "ab12cd34"}

peers = os OUTROS membros do grupo (cada sidecar lista todos menos o dono). gid = id estável do
grupo (não muda quando membro entra/sai) — nomeia o arquivo de CONTRATO compartilhado. Formato
legado {"peer": "x"} (1:1) é lido como {"peers": ["x"]}. O efeito de comportamento (as sessões se
falarem via hangar-send) vem do PROMPT que a API injeta; o sidecar persiste o vínculo pro badge/unpair."""
import hashlib
import json
import logging
import shutil
import threading
import time
import uuid
from pathlib import Path

from app import atomico
from app.adapters.orq import runs as orq_runs
from app.config import settings
from app.models import dumps_safe

_log = logging.getLogger(__name__)
from app.pqueue import _sanitize

# Lock global das operações de GRUPO (N sidecars): join/leave/rename concorrentes sem isto podiam
# deixar listas assimétricas ou ressuscitar vínculo que um leave acabou de limpar. Ação de usuário,
# não hot path. ponytail: lock global; granular se pair/unpair virar gargalo (não vai).
_LOCK = threading.Lock()


class GroupsOwnedByRust(RuntimeError):
    """Escrita de grupo no Python com o Rust dono (`rust`/`pending`): um caminho esquecido falha
    alto em vez de virar segundo escritor da pasta."""


def _refuse_if_rust(what: str) -> None:
    from app import groups_bridge
    if groups_bridge.rust_owns_groups():
        raise GroupsOwnedByRust(what)


class PairMixError(ValueError):
    """Tentativa de misturar pareamento cross-server (1:1) com grupo local — proibido (ver
    join_group). O caller (api.py) traduz pra HTTP 400."""


class TaskConflito(ValueError):
    """O grupo já tem tarefa e o join trouxe outra sem pedir a troca. Cada --pair de um árbitro
    sobrescrevia a tarefa de TODOS os membros calado (a skill orquestrar avisava contra isso na
    doc, que é sinal de que a API devia proteger). O caller traduz pra HTTP 409."""

    def __init__(self, existente: str):
        super().__init__(f"o grupo já tem tarefa: {existente!r}")
        self.existente = existente


def _pair_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".hangar-pair"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _gid_legado(name: str, peers: list[str]) -> str:
    """gid derivado do CONJUNTO de membros, pra sidecar escrito antes de o gid existir (par 1:1).

    Todo membro deriva do mesmo conjunto ordenado, então os sidecars de um mesmo grupo caem no mesmo
    valor sem combinarem nada — que é o que a lista do app precisa pra agrupá-los. Sem isto o link
    volta com `gid` vazio, `clusterByPair` não agrupa e a sessão aparece solta: o desktop disfarçava
    com um chip próprio lendo `peers`, e o celular não mostrava vínculo nenhum.

    Não é gravado: escrever durante uma leitura que roda a cada poll da lista é caro e arriscado, e
    o primeiro `join_group`/`leave` já grava um gid de verdade por cima."""
    return hashlib.sha1("\n".join(sorted([name, *peers])).encode("utf-8")).hexdigest()[:8]


class PairLink:
    """Sidecar de UM membro (<nome>.json). get() normaliza o formato legado 1:1."""

    def __init__(self, name: str):
        self.name = name
        self.path = _pair_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None
        if not isinstance(data, dict):
            return None
        # Legado {"peer": "x"} -> {"peers": ["x"]}; gid ausente ganha um na próxima escrita.
        if "peers" not in data and data.get("peer"):
            data["peers"] = [data["peer"]]
        peers = [p for p in (data.get("peers") or []) if p]
        # Sem peers só o grupo de orquestração existe: o árbitro do `orquestrar-auto` nasce sozinho
        # e o time chega depois, pelo orquestrador.
        if not peers and data.get("orq") is not True:
            return None
        harness = data.get("harness")
        return {"peers": peers, "task": data.get("task", ""),
                "gid": data.get("gid") or _gid_legado(self.name, peers),
                "harness": harness if isinstance(harness, dict) else {},
                "orq": data.get("orq") is True}

    def set(self, peers: list[str], task: str = "", gid: str = "",
            harness: dict[str, str] | None = None, orq: bool = False) -> None:
        """`harness` = nome -> provider do grupo (o hook de SessionStart rotula a lista com ele);
        só ficam as chaves do próprio grupo."""
        _refuse_if_rust(self.name)
        dentro = {self.name, *peers}
        corpo = {"peers": peers, "task": task, "gid": gid,
                 "harness": {n: p for n, p in (harness or {}).items() if n in dentro}}
        if orq:
            corpo["orq"] = True
        # Escrita atômica (tmp + replace), mesmo padrão do PromptQueue._write_atomic.
        tmp = self.path.with_suffix(".json.tmp")
        tmp.write_text(dumps_safe(corpo), encoding="utf-8")
        atomico.substituir(tmp, self.path)

    def clear(self) -> None:
        _refuse_if_rust(self.name)
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".json.tmp").unlink(missing_ok=True)


def _members_of(name: str) -> tuple[list[str], str, str]:
    """(membros INCLUINDO name, task, gid) do grupo de `name`; sem grupo -> ([name], "", "")."""
    link = PairLink(name).get()
    if not link:
        return [name], "", ""
    return [name, *link["peers"]], link["task"], link["gid"]


def _write_group(members: list[str], task: str, gid: str, harness: dict[str, str],
                 orq: bool = False) -> None:
    """Grava o sidecar de CADA membro LOCAL com os demais como peers (chamada já sob _LOCK). Membro
    REMOTO (nome qualificado 'srv::sessao') não tem sidecar aqui — ele vive na máquina dele; entra só
    como string na lista de peers dos locais. O reverso (o sidecar de lá) é escrito pelo backend
    remoto via /pair-remote."""
    for m in members:
        if "::" in m:
            continue
        PairLink(m).set([p for p in members if p != m], task, gid, harness, orq)


def snapshot(names: list[str]) -> dict[str, dict | None]:
    """Estado cru dos sidecars de todos os grupos que tocam `names` (pra restore em rollback)."""
    with _LOCK:
        involved: set[str] = set()
        for n in names:
            involved.update(_members_of(n)[0])
        return {m: PairLink(m).get() for m in involved}


def _restore_locked(snap: dict[str, dict | None]) -> None:
    # Corpo do restore SEM adquirir o lock — pra uso de quem já o segura (join/leave em rollback
    # de escrita parcial). _LOCK não é reentrante; chamar restore() de dentro deadlockava.
    for m, st in snap.items():
        if st is None:
            PairLink(m).clear()
        else:
            PairLink(m).set(st["peers"], st.get("task", ""), st.get("gid", ""), st.get("harness"),
                            st.get("orq", False))


def restore(snap: dict[str, dict | None]) -> None:
    """Desfaz um join que não pôde ser concluído (ex: aviso não entregue): volta cada sidecar
    exatamente ao estado do snapshot."""
    with _LOCK:
        _restore_locked(snap)


def join_group(name: str, others: list[str], task: str = "", substituir_task: bool = False,
               harness: dict[str, str] | None = None,
               orq: bool = False) -> tuple[list[str], dict[str, dict | None]]:
    """Une os grupos de `name` e de CADA sessão em `others` num só (N sessões soltas = grupo novo)
    e devolve (membros finais, snapshot pré-join pra rollback). snapshot+join na MESMA seção
    crítica: em seções separadas, um join concorrente na janela entre elas entrava no grupo sem
    entrar no snapshot — e o restore() de um rollback nunca o reverteria (grupo fantasma parcial).

    task só entra em grupo sem tarefa; diferente da existente exige `substituir_task`, senão
    `TaskConflito`; vazia herda. gid: mantém o primeiro grupo existente (estável pro arquivo de
    contrato); contratos dos grupos absorvidos são ANEXADOS ao sobrevivente (nenhum combinado se
    perde órfão no disco). Escrita parcial (ex: disco cheio no 4º de 5 sidecars) restaura o
    snapshot e propaga — nunca grupo assimétrico."""
    with _LOCK:
        all_names = list(dict.fromkeys([name, *others]))
        infos = [_members_of(n) for n in all_names]
        members = list(dict.fromkeys([m for ms, _, _ in infos for m in ms]))  # união, ordem estável
        # INVARIANTE local/remoto: um par cross-server (nome 'srv::sessao') só existe como 1:1 — UMA
        # sessão local + UM remoto. A união de grupos podia arrastar um remoto pré-existente de um
        # membro pra dentro de um grupo local (join_group('B',['A']) com A já pareada a srv::X vazava
        # srv::X pro sidecar de B), criando vínculo local com um remoto que NUNCA passou pelo handshake
        # /pair-remote — e um unpair depois dissolvia o par legítimo do OUTRO server. Rejeita a mistura
        # aqui (root cause), não só no check de shape do request lá no api.py. Nada foi mutado ainda.
        remotes = [m for m in members if "::" in m]
        locals_ = [m for m in members if "::" not in m]
        if remotes and (len(remotes) > 1 or len(locals_) != 1):
            raise PairMixError(
                "pareamento cross-server é 1:1 (uma sessão local + um peer remoto); uma sessão já "
                "pareada cross-server não entra em grupo local nem pareia com outro remoto")
        snap = {m: PairLink(m).get() for m in members}
        existente = next((t for _, t, _ in infos if t), "")
        pedida = task.strip()
        if pedida and existente and pedida != existente and not substituir_task:
            raise TaskConflito(existente)   # antes de qualquer mutação
        final_task = pedida if (pedida and (not existente or substituir_task)) else existente
        gids = list(dict.fromkeys([g for _, _, g in infos if g]))
        gid = gids[0] if gids else uuid.uuid4().hex[:8]
        for loser in gids[1:]:
            _merge_contract(loser_gid=loser, survivor_gid=gid)
        # O que os sidecars já sabiam vale até o chamador trazer o provider atual.
        todos: dict[str, str] = {}
        for st in snap.values():
            todos.update((st or {}).get("harness") or {})
        todos.update(harness or {})
        # Grupo de orquestração continua sendo, mesmo quando o join seguinte não repete a marca.
        orq = orq or any((st or {}).get("orq") for st in snap.values())
        try:
            _write_group(members, final_task, gid, todos, orq)
            if orq and not gids:
                from app import orq_context
                try:
                    orq_context.promote(name, gid)
                except orq_context.IdentityUnavailable:
                    # Sem identidade não houve configuração pela API para promover.
                    _log.warning("pair: sem identidade para promover o grupo %s", gid)
        except OSError:
            _restore_locked(snap)
            raise
        except ValueError:
            _restore_locked(snap)
            raise
        return members, snap


def join_with_snapshot(a: str, b: str, task: str = "") -> tuple[list[str], dict[str, dict | None]]:
    """Atalho de join_group pra um par (compat com callers/testes do modelo 2-a-2)."""
    return join_group(a, [b], task)


def _merge_contract(loser_gid: str, survivor_gid: str) -> None:
    """Anexa o contrato do grupo absorvido ao do sobrevivente (best-effort; merge de grupos não
    pode falhar por causa de arquivo de contrato)."""
    _refuse_if_rust(loser_gid)
    # `regras-` (o que o time lê) segue o `grupo-` (o registro do árbitro): sem isto o merge
    # deixava o regras-<loser> órfão, o mesmo furo que o leave() já fechava só pro grupo-.
    for prefixo in ("grupo", "regras"):
        loser = _pair_dir() / f"{prefixo}-{loser_gid}.md"
        # Guarda ANTES do try, igual ao _arquivar_contratos: o arquivo de contrato só existe se
        # alguém escreveu um, então "não há o que herdar" é o caso comum de um merge — e sem isto o
        # FileNotFoundError (que é OSError) cairia no warning abaixo, 2x por fusão, chamando de
        # falha o caminho normal.
        if not loser.is_file():
            continue
        try:
            content = loser.read_text(encoding="utf-8").strip()
            if not content:
                continue
            survivor = _pair_dir() / f"{prefixo}-{survivor_gid}.md"
            old = survivor.read_text(encoding="utf-8") if survivor.exists() else ""
            survivor.write_text(
                old + f"\n\n## Contrato herdado do grupo {loser_gid} (merge)\n\n" + content + "\n",
                encoding="utf-8")
            loser.unlink(missing_ok=True)
        except OSError as e:
            # Best-effort de propósito (falhar aqui não desfaz um merge que já valeu), mas não pode
            # ser MUDO: sem rastro, um contrato herdado que sumiu vira mistério.
            _log.warning("merge de contrato falhou prefixo=%s perdedor=%s sobrevivente=%s: %s",
                         prefixo, loser_gid, survivor_gid, e)


def join(a: str, b: str, task: str = "") -> list[str]:
    """Atalho de join_with_snapshot pra quem não precisa do snapshot (testes/uso simples)."""
    return join_with_snapshot(a, b, task)[0]


def _arquivo_dir() -> Path:
    # O cofre (~/.hangar), não o config dir de uma conta — mesma razão do orq.raiz_padrao().
    return Path.home() / ".hangar" / "pair-arquivo"


def _arquivar_contratos(gid: str) -> None:
    """Último membro saiu: o contrato vai pro arquivo em vez de sumir — dois kills seguidos apagavam
    as decisões de um trabalho inteiro. Best-effort: falha aqui não desfaz um leave que já valeu."""
    _refuse_if_rust(gid)
    ts = time.strftime("%Y%m%d-%H%M%S")
    for prefixo in ("grupo", "regras"):
        src = _pair_dir() / f"{prefixo}-{gid}.md"
        try:
            if not src.is_file():
                continue
            dst = _arquivo_dir()
            dst.mkdir(parents=True, exist_ok=True)
            shutil.move(str(src), str(dst / f"{prefixo}-{gid}-{ts}.md"))
        except OSError as e:
            _log.warning("arquivamento de contrato falhou prefixo=%s gid=%s: %s", prefixo, gid, e)


def leave(name: str) -> list[str]:
    """`name` sai do grupo. Devolve os ex-companheiros (pra notificação). Grupo restante de 1
    também é dissolvido (grupo de 1 não existe), salvo o de orquestração com execução `auto` viva.
    Idempotente. Escrita parcial (ex: OSError no 2º companheiro) restaura o estado anterior e
    propaga — sem isto sobrava companheiro-fantasma apontando pra quem já saiu.

    Grupo dissolvido de vez (ninguém mais dentro) arquiva o contrato em `~/.hangar/pair-arquivo/`
    (sem sidecar apontando pra ele, no lugar original seria órfão; apagar perdia decisões)."""
    with _LOCK:
        link = PairLink(name).get()
        if not link:
            return []
        peers = link["peers"]
        # Execução `orquestrar-auto` viva: o orquestrador fecha e abre as sessões das Tasks, e o
        # grupo passa por trechos só com o árbitro. Ilegível conta como viva: desfazer não tem volta.
        vivo = link["orq"] and orq_runs.group_phase(link["gid"]) in ("live", "unknown")
        snap = {m: PairLink(m).get() for m in [name, *peers]}
        try:
            PairLink(name).clear()
            if len(peers) == 1 and not vivo:
                PairLink(peers[0]).clear()
            else:
                for p in peers:
                    st = PairLink(p).get()
                    if st:
                        PairLink(p).set([x for x in st["peers"] if x != name],
                                        st.get("task", ""), st.get("gid", ""), st.get("harness"),
                                        st.get("orq", False))
        except OSError:
            _restore_locked(snap)
            raise
        # Fora do try: arquivar contrato é faxina, best-effort — falha aqui não pode desfazer um
        # unpair que já deu certo (restore ressuscitaria o par).
        # Com a execução viva o orquestrador ainda lê o `regras-<gid>.md` a cada kick-off.
        if len(peers) <= 1 and link.get("gid") and not vivo:
            _arquivar_contratos(link["gid"])
        return peers


# Do `--pair --orq` do árbitro ao `execucao_inicio`: contrato, fechamento e `orq init` no meio.
ORQ_LAUNCH_GRACE_S = 3600


def dissolve_lone_orq() -> list[str]:
    """Grupo `orq` de um membro só vive enquanto a execução `auto` dele vive. Acabada (ou nunca
    iniciada depois da janela de lançamento), o sidecar sai e o contrato vai pro arquivo: o hook de
    SessionStart lê o arquivo do sidecar e reinjetaria o protocolo pra sempre."""
    out = []
    with _LOCK:
        for f in _pair_dir().glob("*.json"):
            st = PairLink(f.stem).get()
            if not st or st["peers"] or not st["orq"]:
                continue
            fase = orq_runs.group_phase(st["gid"])
            try:
                novo = time.time() - f.stat().st_mtime < ORQ_LAUNCH_GRACE_S
            except OSError:
                continue
            if fase in ("live", "unknown") or (fase is None and novo):
                continue
            PairLink(f.stem).clear()
            _arquivar_contratos(st["gid"])
            out.append(f.stem)
    return out


def rename_pair(old: str, new: str) -> None:
    """Sessão renomeada: migra o próprio sidecar E re-escreve a lista de cada companheiro."""
    with _LOCK:
        link = PairLink(old).get()
        if not link:
            PairLink(old).clear()
            return
        PairLink(old).clear()
        def renomeado(h: dict[str, str] | None) -> dict[str, str]:
            return {(new if n == old else n): p for n, p in (h or {}).items()}
        PairLink(new).set(link["peers"], link.get("task", ""), link.get("gid", ""),
                          renomeado(link.get("harness")), link.get("orq", False))
        for p in link["peers"]:
            st = PairLink(p).get()
            if st:
                PairLink(p).set([new if x == old else x for x in st["peers"]],
                                st.get("task", ""), st.get("gid", ""), renomeado(st.get("harness")),
                                st.get("orq", False))


def referenciados_locais() -> set[str]:
    """Nomes LOCAIS que têm sidecar ou aparecem como peer em algum — os candidatos a fantasma
    quando a sessão morre fora do app. O dono entra porque num par cross-server ele é o único
    local (o peer é 'srv::x', que não se vê daqui e fica de fora)."""
    with _LOCK:
        out: set[str] = set()
        for f in _pair_dir().glob("*.json"):
            st = PairLink(f.stem).get()
            if st:
                out.add(f.stem)
                out.update(p for p in st["peers"] if "::" not in p)
        return out


def contract_path_for(name: str) -> Path | None:
    """Arquivo de CONTRATO do grupo de `name` (markdown, keyed pelo gid — estável quando membro
    entra/sai). Todos os membros derivam o mesmo path; editam via fs; o app exibe no PairSheet.
    Sobrevive a membro que sai sozinho; some junto com o grupo quando o último sai (ver leave()).
    None = sem grupo."""
    link = PairLink(name).get()
    if not link or not link.get("gid"):
        return None
    return _pair_dir() / f"grupo-{link['gid']}.md"
