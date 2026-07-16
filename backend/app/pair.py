"""Pareamento de sessões (feature "trabalhando juntas"): GRUPO de N sessões que colaboram em repos
complementares (ex: front + back + POS). Mesmo padrão de sidecar do ThenLink (app/chain.py): dir
irmão ".claude-pocket-pair", um JSON pequeno por MEMBRO, keyed pelo NOME:

    {"peers": ["outra", "mais-uma"], "task": "...", "gid": "ab12cd34"}

peers = os OUTROS membros do grupo (cada sidecar lista todos menos o dono). gid = id estável do
grupo (não muda quando membro entra/sai) — nomeia o arquivo de CONTRATO compartilhado. Formato
legado {"peer": "x"} (1:1) é lido como {"peers": ["x"]}. O efeito de comportamento (as sessões se
falarem via cp-send) vem do PROMPT que a API injeta; o sidecar persiste o vínculo pro badge/unpair."""
import json
import threading
import uuid
from pathlib import Path

from app.config import settings
from app.pqueue import _sanitize

# Lock global das operações de GRUPO (N sidecars): join/leave/rename concorrentes sem isto podiam
# deixar listas assimétricas ou ressuscitar vínculo que um leave acabou de limpar. Ação de usuário,
# não hot path. ponytail: lock global; granular se pair/unpair virar gargalo (não vai).
_LOCK = threading.Lock()


def _pair_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-pair"
    d.mkdir(parents=True, exist_ok=True)
    return d


class PairLink:
    """Sidecar de UM membro (<nome>.json). get() normaliza o formato legado 1:1."""

    def __init__(self, name: str):
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
        if not peers:
            return None
        return {"peers": peers, "task": data.get("task", ""), "gid": data.get("gid", "")}

    def set(self, peers: list[str], task: str = "", gid: str = "") -> None:
        # Escrita atômica (tmp + replace), mesmo padrão do PromptQueue._write_atomic.
        tmp = self.path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps({"peers": peers, "task": task, "gid": gid}, ensure_ascii=False),
                       encoding="utf-8")
        tmp.replace(self.path)

    def clear(self) -> None:
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".json.tmp").unlink(missing_ok=True)


def _members_of(name: str) -> tuple[list[str], str, str]:
    """(membros INCLUINDO name, task, gid) do grupo de `name`; sem grupo -> ([name], "", "")."""
    link = PairLink(name).get()
    if not link:
        return [name], "", ""
    return [name, *link["peers"]], link["task"], link["gid"]


def _write_group(members: list[str], task: str, gid: str) -> None:
    """Grava o sidecar de CADA membro com os demais como peers (chamada já sob _LOCK)."""
    for m in members:
        PairLink(m).set([p for p in members if p != m], task, gid)


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
            PairLink(m).set(st["peers"], st.get("task", ""), st.get("gid", ""))


def restore(snap: dict[str, dict | None]) -> None:
    """Desfaz um join que não pôde ser concluído (ex: aviso não entregue): volta cada sidecar
    exatamente ao estado do snapshot."""
    with _LOCK:
        _restore_locked(snap)


def join_group(name: str, others: list[str], task: str = "") -> tuple[list[str], dict[str, dict | None]]:
    """Une os grupos de `name` e de CADA sessão em `others` num só (N sessões soltas = grupo novo)
    e devolve (membros finais, snapshot pré-join pra rollback). snapshot+join na MESMA seção
    crítica: em seções separadas, um join concorrente na janela entre elas entrava no grupo sem
    entrar no snapshot — e o restore() de um rollback nunca o reverteria (grupo fantasma parcial).

    task informada substitui a anterior; vazia herda a primeira existente. gid: mantém o primeiro
    grupo existente (estável pro arquivo de contrato); contratos dos grupos absorvidos são
    ANEXADOS ao sobrevivente (nenhum combinado se perde órfão no disco). Escrita parcial (ex:
    disco cheio no 4º de 5 sidecars) restaura o snapshot e propaga — nunca grupo assimétrico."""
    with _LOCK:
        all_names = list(dict.fromkeys([name, *others]))
        infos = [_members_of(n) for n in all_names]
        members = list(dict.fromkeys([m for ms, _, _ in infos for m in ms]))  # união, ordem estável
        snap = {m: PairLink(m).get() for m in members}
        final_task = task.strip() or next((t for _, t, _ in infos if t), "")
        gids = list(dict.fromkeys([g for _, _, g in infos if g]))
        gid = gids[0] if gids else uuid.uuid4().hex[:8]
        for loser in gids[1:]:
            _merge_contract(loser_gid=loser, survivor_gid=gid)
        try:
            _write_group(members, final_task, gid)
        except OSError:
            _restore_locked(snap)
            raise
        return members, snap


def join_with_snapshot(a: str, b: str, task: str = "") -> tuple[list[str], dict[str, dict | None]]:
    """Atalho de join_group pra um par (compat com callers/testes do modelo 2-a-2)."""
    return join_group(a, [b], task)


def _merge_contract(loser_gid: str, survivor_gid: str) -> None:
    """Anexa o contrato do grupo absorvido ao do sobrevivente (best-effort; merge de grupos não
    pode falhar por causa de arquivo de contrato)."""
    try:
        loser = _pair_dir() / f"grupo-{loser_gid}.md"
        content = loser.read_text(encoding="utf-8").strip()
        if not content:
            return
        survivor = _pair_dir() / f"grupo-{survivor_gid}.md"
        old = survivor.read_text(encoding="utf-8") if survivor.exists() else ""
        survivor.write_text(
            old + f"\n\n## Contrato herdado do grupo {loser_gid} (merge)\n\n" + content + "\n",
            encoding="utf-8")
        loser.unlink(missing_ok=True)
    except OSError:
        pass


def join(a: str, b: str, task: str = "") -> list[str]:
    """Atalho de join_with_snapshot pra quem não precisa do snapshot (testes/uso simples)."""
    return join_with_snapshot(a, b, task)[0]


def leave(name: str) -> list[str]:
    """`name` sai do grupo. Devolve os ex-companheiros (pra notificação). Grupo restante de 1
    também é dissolvido (grupo de 1 não existe). Idempotente. Escrita parcial (ex: OSError no
    2º companheiro) restaura o estado anterior e propaga — sem isto sobrava companheiro-fantasma
    apontando pra quem já saiu."""
    with _LOCK:
        link = PairLink(name).get()
        if not link:
            return []
        peers = link["peers"]
        snap = {m: PairLink(m).get() for m in [name, *peers]}
        try:
            PairLink(name).clear()
            if len(peers) == 1:
                PairLink(peers[0]).clear()
            else:
                for p in peers:
                    st = PairLink(p).get()
                    if st:
                        PairLink(p).set([x for x in st["peers"] if x != name],
                                        st.get("task", ""), st.get("gid", ""))
        except OSError:
            _restore_locked(snap)
            raise
        return peers


def rename_pair(old: str, new: str) -> None:
    """Sessão renomeada: migra o próprio sidecar E re-escreve a lista de cada companheiro."""
    with _LOCK:
        link = PairLink(old).get()
        if not link:
            PairLink(old).clear()
            return
        PairLink(old).clear()
        PairLink(new).set(link["peers"], link.get("task", ""), link.get("gid", ""))
        for p in link["peers"]:
            st = PairLink(p).get()
            if st:
                PairLink(p).set([new if x == old else x for x in st["peers"]],
                                st.get("task", ""), st.get("gid", ""))


def contract_path_for(name: str) -> Path | None:
    """Arquivo de CONTRATO do grupo de `name` (markdown, keyed pelo gid — estável quando membro
    entra/sai). Todos os membros derivam o mesmo path; editam via fs; o app exibe no PairSheet.
    NÃO é apagado no leave (registro do que foi combinado). None = sem grupo."""
    link = PairLink(name).get()
    if not link or not link.get("gid"):
        return None
    return _pair_dir() / f"grupo-{link['gid']}.md"
