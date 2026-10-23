"""Golden dos fatos da lista que o Rust passou a ler do disco (`list/local_facts.rs`,
`list/local_transfers.rs`), tirado do Python que os calculava em `list_facts.py`:

- `guests`: `guest_users.owner_name`/`visible_to` (o `_access`), com a vida de cada sessão dada;
- `orq`: `orq_runs.active()` + `activity()` (o `_orq_rows`), tempos como deslocamento do agora;
- `transfers`: `registry._decorate_transfers` (o `_transfers`), caminhos com `⟦HOME⟧`.

Cada caso parte de um HOME temporário vazio, grava os arquivos de `files` e roda o Python. O Rust
repete os casos em `src/list/local_facts/tests.rs` com os mesmos arquivos.

Uso, de backend/: uv run python tests/fixtures/contract/list_facts/gen_list_facts.py
"""
import json
import os
import subprocess
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
from app import conversation_transfer as store  # noqa: E402
from app import guest_users, registry  # noqa: E402
from app.adapters.orq import runs as orq_runs  # noqa: E402
from app.models import SessionInfo  # noqa: E402

HOME = "⟦HOME⟧"
GUEST = {"id": "g1", "name": "ana", "token_hash": "h", "root": "/r"}

GUESTS = [
    {"case": "sem_arquivo", "file": None, "names": {"a": "k:1"}},
    {"case": "dono_escondido", "file": {"guests": [{**GUEST, "owner_sees": False}],
                                        "sessions": {"a": {"guest": "g1", "life": "k:1"}}},
     "names": {"a": "k:1", "b": "k:2"}},
    {"case": "outra_vida", "file": {"guests": [GUEST], "sessions": {"a": {"guest": "g1", "life": "k:1"}}},
     "names": {"a": "k:9"}},
    {"case": "vida_gravada_vazia", "file": {"guests": [GUEST], "sessions": {"a": {"guest": "g1", "life": None}}},
     "names": {"a": "k:1"}},
    {"case": "vida_desconhecida", "file": {"guests": [GUEST], "sessions": {"a": {"guest": "g1", "life": "k:1"}}},
     "names": {"a": None}},
    {"case": "convidado_sumido", "file": {"guests": [GUEST], "sessions": {"a": {"guest": "g2", "life": "k:1"}}},
     "names": {"a": "k:1"}},
    {"case": "campo_de_sobra", "file": {"guests": [{**GUEST, "x": 1}], "sessions": {"a": {"guest": "g1", "life": "k:1"}}},
     "names": {"a": "k:1"}},
]


def run_guests(case: dict) -> dict:
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "guests.json"
        if case["file"] is not None:
            path.write_text(json.dumps(case["file"]), encoding="utf-8")
        with patch.object(guest_users, "_path_override", path), \
             patch.object(guest_users, "_life", lambda n: case["names"][n]), \
             patch.object(guest_users, "_save", lambda: None):
            guest_users._reset()
            names = set(case["names"])
            if not guest_users.has_claims():
                owners, hidden = {}, []
            else:
                owners = {n: o for n in names if (o := guest_users.owner_name(n))}
                hidden = sorted(n for n in names if not guest_users.visible_to(None, n))
        guest_users._reset()
    return {**case, "owners": owners, "hidden": hidden}


def run(gid: str, *, auto=True, end=False, beat=-10.0, interval=60, arbiter=None, cfg_arbiter="arb",
        timeline=-5.0) -> dict:
    """Pasta de execução: `beat`/`timeline` são deslocamentos (s) do batimento e do mtime da linha do tempo."""
    events = [{"tipo": "execucao_inicio", "gid": gid}] + ([{"tipo": "execucao_fim"}] if end else [])
    hb = {"ts": beat, "interval_s": interval}
    if arbiter:
        hb["arbiter"] = arbiter
    return {"dir": f"run-{gid}", "orq": {"auto": auto, "repo": "/repo", "arbiter": cfg_arbiter},
            "events": events, "heartbeat": hb, "timeline": timeline}


ORQ = [
    {"case": "viva_trabalhando", "runs": [run("g1")]},
    {"case": "viva_parada", "runs": [run("g2", timeline=-600.0, arbiter="hb-arb")]},
    {"case": "vigia_parado", "runs": [run("g3", beat=-1000.0)]},
    {"case": "encerrada", "runs": [run("g4", end=True)]},
    {"case": "nao_auto", "runs": [run("g5", auto=False)]},
    {"case": "duas", "runs": [run("g7"), run("g6", timeline=-300.0)]},
]


def write_run(root: Path, r: dict, now: float) -> None:
    d = root / r["dir"]
    d.mkdir(parents=True)
    (d / "orq.json").write_text(json.dumps(r["orq"]), encoding="utf-8")
    (d / "eventos.jsonl").write_text("".join(json.dumps(e) + "\n" for e in r["events"]), encoding="utf-8")
    hb = {**r["heartbeat"], "ts": datetime.fromtimestamp(now + r["heartbeat"]["ts"]).astimezone().isoformat()}
    (d / "vigia.json").write_text(json.dumps(hb), encoding="utf-8")
    tl = d / f"timeline-{d.name}.jsonl"
    tl.write_text("{}\n", encoding="utf-8")
    os.utime(tl, (now + r["timeline"], now + r["timeline"]))


def run_orq(case: dict) -> dict:
    with tempfile.TemporaryDirectory() as d:
        root = Path(d) / ".hangar" / "orq"
        root.mkdir(parents=True)
        now = time.time()
        for r in case["runs"]:
            write_run(root, r, now)
        with patch.object(orq_runs, "root", lambda: root):
            out = []
            for r in orq_runs.active():
                state, last = orq_runs.activity(r["timeline"])
                out.append({"name": r["name"], "cwd": r["repo"], "jsonl": r["timeline"].replace(d, HOME),
                            "pair_gid": r["gid"], "orq_arbiter": r["arbiter"], "state": state,
                            "last_activity": None if last is None else round(last - now, 3)})
    return {**case, "rows": out}


def record(rid: str, name: str, phase: str, *, life="k:old", dest_life="k:new", thread="th-1",
           origin_jsonl="⟦HOME⟧/src.jsonl", error=None) -> dict:
    return {"id": rid, "name": name, "source_life": life, "phase": phase,
            "source": {"path": origin_jsonl, "provider": "claude", "digest": "d" * 64, "selected_uuids": ["u0"]},
            "origin_meta": {"name": name, "cwd": "/w", "session_id": "src", "key": "kk", "jsonl": origin_jsonl,
                            "headless": True, "engine": None, "config_dir": "⟦HOME⟧/.claude"},
            "destination_meta": {"thread_id": thread, "codex_home": "⟦HOME⟧/.codex", "lifecycle_id": dest_life, "key": "new"},
            "boundary": None, "error_code": error}


IDS = ["11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222"]
TRANSFERS = [
    {"case": "em_curso_na_linha", "records": [record(IDS[0], "fx-a", "source_stopped")], "sidecars": {},
     "rows": [{"name": "fx-a", "provider": "claude", "headless": True, "lifecycle_id": "k:old", "jsonl": "⟦HOME⟧/src.jsonl"}]},
    {"case": "em_curso_sem_linha", "records": [record(IDS[0], "fx-b", "restore_failed", error="transfer_restore_failed")],
     "sidecars": {}, "rows": []},
    {"case": "completa_pela_vida_nova", "records": [record(IDS[0], "fx-c", "complete")], "sidecars": {},
     "rows": [{"name": "fx-c", "provider": "claude", "lifecycle_id": "k:new"}]},
    {"case": "recriada_com_o_nome", "records": [record(IDS[0], "fx-d", "source_stopped")], "sidecars": {},
     "rows": [{"name": "fx-d", "provider": "claude", "lifecycle_id": "k:outra"}]},
    {"case": "codex_pela_thread", "records": [record(IDS[0], "fx-e", "imported")],
     "sidecars": {"codex-sessions/fx-e.json": {"name": "fx-e", "thread_id": "th-1", "codex_home": "⟦HOME⟧/.codex"}},
     "rows": [{"name": "fx-e", "provider": "codex", "codex_home": "⟦HOME⟧/.codex", "lifecycle_id": "k:x"}]},
    {"case": "sidecar_aponta_ausente", "records": [],
     "sidecars": {"claude-headless/fx-f.json": {"name": "fx-f", "transfer_id": IDS[1]}},
     "rows": [{"name": "fx-f", "provider": "claude", "headless": True, "lifecycle_id": "k:old"}]},
]


def expand(v, home: str):
    if isinstance(v, str):
        return v.replace(HOME, home)
    if isinstance(v, dict):
        return {k: expand(x, home) for k, x in v.items()}
    if isinstance(v, list):
        return [expand(x, home) for x in v]
    return v


FIELDS = ("name", "cwd", "jsonl", "provider", "headless", "engine", "codex_home", "conta", "lifecycle_id",
          "transfer_id", "transfer_phase", "problema")


def run_transfers(case: dict) -> dict:
    from app.conversation_transfer import TransferPhase
    terminal = {p.value for p in (TransferPhase.COMPLETE, TransferPhase.REJECTED, TransferPhase.ROLLED_BACK)}
    with tempfile.TemporaryDirectory() as d:
        home = str(Path(d).resolve())
        with patch.dict(os.environ, {"HOME": home}):
            for r in case["records"]:
                store.save_transfer(store._decode(expand(r, home)))
            for rel, meta in case["sidecars"].items():
                p = Path(home) / ".hangar" / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(json.dumps(expand(meta, home)), encoding="utf-8")
            files = {str(p.relative_to(home)): p.read_text(encoding="utf-8").replace(home, HOME)
                     for p in sorted(Path(home).rglob("*.json")) if p.name != ".lock"}
            infos = [SessionInfo.model_validate(expand(r, home)) for r in case["rows"]]
            try:
                registry._decorate_transfers(infos)
            except ValueError:
                return {**case, "files": files, "error": True}
            touched = [i for i in infos if i.transfer_id]
            dumps = [i.model_dump(mode="json") for i in touched]
            out = [{f: v.replace(home, HOME) if isinstance(v := row[f], str) else v for f in FIELDS} for row in dumps]
            frozen = sorted(i.name for i in touched if i.transfer_phase and i.transfer_phase not in terminal)
    return {**case, "files": files, "error": False, "touched": out, "frozen": frozen}


def main() -> None:
    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    out = {
        "origin_commit": commit,
        "command": "uv run python tests/fixtures/contract/list_facts/gen_list_facts.py",
        "guests": [run_guests(c) for c in GUESTS],
        "orq": [run_orq(c) for c in ORQ],
        "transfers": [run_transfers(c) for c in TRANSFERS],
        # O nome do índice `thread-*` depende do caminho do HOME: o Rust renomeia ao repetir, e a chave
        # que ele calcula é conferida contra esta.
        "thread_key": [{"home": h, "thread": t, "key": store._thread_key(h, t)}
                       for h, t in [("/x/.codex", "th-1"), ("/x/a b/.codex/", "ç")]],
    }
    (HERE / "list_facts.json").write_text(json.dumps(out, ensure_ascii=False, indent=1, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
