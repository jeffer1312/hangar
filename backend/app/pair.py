"""Pareamento de sessões (feature "trabalhando juntas"): vínculo SIMÉTRICO entre duas sessões que
colaboram em repos complementares (ex: front + back). Mesmo padrão de sidecar do ThenLink
(app/chain.py): dir irmão ".claude-pocket-pair", um JSON pequeno por sessão, keyed pelo NOME.
Simetria é responsabilidade do caller (api.pair/unpair grava/limpa os DOIS lados). O efeito de
comportamento (as sessões passarem a se falar via cp-send) vem do PROMPT de pareamento que a API
injeta em ambas — o sidecar só persiste o vínculo pro badge da UI e pro unpair."""
import json
import threading
from pathlib import Path

from app.config import settings
from app.pqueue import _sanitize

# Lock global das operacoes SIMETRICAS (par de sidecars): pair/unpair/rename concorrentes sem isto
# podiam deixar assimetria (A aponta B, B aponta C) ou ressuscitar link que o unpair acabou de
# limpar (get+set do rename intercalado com o clear). Acao de usuario, nao hot path.
# ponytail: lock global; granular por sessao se pair/unpair virar gargalo (nao vai).
_LOCK = threading.Lock()


def _pair_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-pair"
    d.mkdir(parents=True, exist_ok=True)
    return d


class PairLink:
    """Vínculo de pareamento de UMA sessão (sidecar <nome>.json com {"peer","task"})."""

    def __init__(self, name: str):
        self.path = _pair_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            return json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None

    def set(self, peer: str, task: str = "") -> None:
        # Escrita atômica (tmp + replace), mesmo padrão do PromptQueue._write_atomic.
        tmp = self.path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps({"peer": peer, "task": task}, ensure_ascii=False),
                       encoding="utf-8")
        tmp.replace(self.path)

    def clear(self) -> None:
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".json.tmp").unlink(missing_ok=True)

    def rename(self, new_name: str) -> None:
        # Espelha ThenLink.rename: migra o próprio sidecar. O sidecar do PAR (que aponta pro nome
        # velho) NÃO é migrado aqui — quem renomeia (registry.rename) atualiza o ponteiro do par.
        self.path.with_suffix(".json.tmp").unlink(missing_ok=True)
        if self.path.exists():
            self.path.replace(_pair_dir() / f"{_sanitize(new_name)}.json")


def pair_both(a: str, b: str, task: str = "") -> None:
    """Grava o vínculo SIMÉTRICO (2 sidecars) sob lock, com rollback: se o lado B falhar, o lado A
    é desfeito — nunca fica meio-pareado invisível (badge em A sem par de volta em B)."""
    with _LOCK:
        PairLink(a).set(b, task)
        try:
            PairLink(b).set(a, task)
        except OSError:
            PairLink(a).clear()
            raise


def unpair_both(name: str) -> str | None:
    """Limpa o vínculo dos DOIS lados sob lock. Devolve o peer (pra notificação) ou None se não
    estava pareada. Idempotente."""
    with _LOCK:
        link = PairLink(name).get()
        if not link:
            return None
        peer = link.get("peer") or None
        PairLink(name).clear()
        if peer:
            PairLink(peer).clear()
        return peer


def rename_pair(old: str, new: str) -> None:
    """Migra o sidecar da sessão renomeada E re-aponta o do par, sob o MESMO lock — sem ele, um
    unpair concorrente podia ser sobrescrito pelo re-aponte (link ressuscitado)."""
    with _LOCK:
        PairLink(old).rename(new)
        pair = PairLink(new).get()
        if pair and pair.get("peer"):
            PairLink(pair["peer"]).set(new, pair.get("task", ""))


def contract_path(a: str, b: str) -> Path:
    """Arquivo de CONTRATO compartilhado do par (markdown): os DOIS lados derivam o mesmo path
    (nomes ordenados), editam direto via fs (têm acesso ao disco) e o app exibe no PairSheet.
    Vive no mesmo dir dos sidecars; NÃO é apagado no unpair (registro do que foi combinado)."""
    x, y = sorted((_sanitize(a), _sanitize(b)))
    return _pair_dir() / f"{x}__{y}.md"
