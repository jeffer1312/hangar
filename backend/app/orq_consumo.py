"""Snapshots locais de consumo por papel; uso: python -m app.orq_consumo --help."""
from __future__ import annotations

import argparse
import hashlib
import json
import socket
import sys
from datetime import datetime, timezone
from pathlib import Path

from app.stats import Accumulator

TOKENS = ("input", "cache_read", "cache_write", "output")
COUNTERS = (*TOKENS, "steps", "bytes")
PROVIDERS = ("claude", "codex")


def _digest(path: Path, size: int) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        remaining = size
        while remaining:
            chunk = stream.read(min(remaining, 1024 * 1024))
            if not chunk:
                raise ValueError(f"Fonte truncada: {path}")
            digest.update(chunk)
            remaining -= len(chunk)
    return digest.hexdigest()


def snapshot(provider: str, transcript: str, role: str, session: str,
             model: str | None = None, effort: str | None = None) -> dict:
    path = Path(transcript)
    if not path.is_absolute() or not role.strip() or not session.strip():
        raise ValueError("Informe transcript absoluto, papel e sessão.")
    path = path.resolve(strict=True)
    before = path.stat()
    accumulator = Accumulator.for_provider(provider, str(path))
    if accumulator is None:
        raise ValueError(f"Provider não suportado: {provider}")
    totals = accumulator.usage_totals()
    if any(totals[key] < 0 for key in COUNTERS):
        raise ValueError("Fonte contém contadores de uso inconsistentes.")
    digest = _digest(path, totals["bytes"])
    after = path.stat()
    if ((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) !=
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)):
        raise ValueError("Fonte mudou durante a coleta; repita quando a sessão estiver ociosa.")
    if after.st_size != totals["bytes"]:
        raise ValueError("Fonte contém linha parcial; repita quando a escrita terminar.")
    return {
        "version": 1, "ts": datetime.now(timezone.utc).isoformat(),
        "role": role, "session": session, "provider": provider,
        "model_observed": model, "effort_observed": effort,
        "source": {"host": socket.gethostname(), "path": str(path),
                   "device": after.st_dev, "inode": after.st_ino},
        "sha256": digest, "totals": totals,
        "coverage": "uso_observado" if totals["steps"] else "sem_uso_observado",
    }


def _validate(value: dict) -> datetime:
    if not isinstance(value, dict) or type(value.get("version")) is not int or value["version"] != 1:
        raise ValueError("Snapshot inválido ou versão desconhecida.")
    for key in ("role", "session", "provider", "ts", "sha256"):
        if not isinstance(value.get(key), str) or not value[key].strip():
            raise ValueError(f"Snapshot sem {key} válido.")
    if value["provider"] not in PROVIDERS:
        raise ValueError("Provider não suportado no snapshot.")
    totals, source = value.get("totals"), value.get("source")
    if not isinstance(totals, dict) or not isinstance(source, dict):
        raise ValueError("Snapshot sem totais ou fonte.")
    for key in COUNTERS:
        if type(totals.get(key)) is not int or totals[key] < 0:
            raise ValueError(f"Contador inválido: {key}")
    if totals["steps"] == 0 and any(totals[key] for key in TOKENS):
        raise ValueError("Snapshot tem tokens sem chamadas observadas.")
    for key in ("device", "inode"):
        if type(source.get(key)) is not int or source[key] < 0:
            raise ValueError(f"Identidade da fonte inválida: {key}")
    if source.get("host") != socket.gethostname():
        raise ValueError("O relatório precisa rodar na máquina que coletou os snapshots.")
    if not isinstance(source.get("path"), str) or not Path(source["path"]).is_absolute():
        raise ValueError("Snapshot sem caminho absoluto da fonte.")
    stamp = datetime.fromisoformat(value["ts"])
    if stamp.tzinfo is None:
        raise ValueError("Timestamp precisa de fuso horário.")
    path = Path(source["path"])
    stat = path.stat()
    if (str(path.resolve(strict=True)) != str(path) or
            (stat.st_dev, stat.st_ino) != (source["device"], source["inode"])):
        raise ValueError(f"Fonte substituída: {path}")
    if _digest(path, totals["bytes"]) != value["sha256"]:
        raise ValueError(f"Fonte reescrita: {path}")
    return stamp


def report(pairs: list[tuple[dict, dict]]) -> dict:
    if not pairs:
        raise ValueError("Informe pelo menos um par de snapshots.")
    intervals: list[dict] = []
    roles: dict[str, dict] = {}
    seen: dict[tuple, list[tuple]] = {}
    for start, end in pairs:
        start_ts, end_ts = _validate(start), _validate(end)
        for key in ("source", "provider", "role", "session"):
            if start[key] != end[key]:
                raise ValueError(f"Snapshots de fontes ou vínculos diferentes: {key}")
        if end_ts <= start_ts:
            raise ValueError("O fim precisa ser posterior ao início.")
        delta = {key: end["totals"][key] - start["totals"][key] for key in COUNTERS}
        if any(number < 0 for number in delta.values()):
            raise ValueError("Contadores regrediram entre os snapshots.")
        if delta["steps"] == 0 or delta["bytes"] == 0 or not any(delta[k] for k in TOKENS):
            raise ValueError("Sem uso novo observado; não há consumo comprovado para este par.")
        source = start["source"]
        identity = (source["host"], source["device"], source["inode"])
        first, last = start["totals"]["bytes"], end["totals"]["bytes"]
        for t0, t1, b0, b1 in seen.get(identity, []):
            if (start_ts < t1 and end_ts > t0) or (first < b1 and last > b0):
                raise ValueError(f"Intervalos sobrepostos da mesma fonte: {source['path']}")
        seen.setdefault(identity, []).append((start_ts, end_ts, first, last))
        tokens = {key: delta[key] for key in TOKENS}
        seconds = (end_ts - start_ts).total_seconds()
        interval = {"role": start["role"], "session": start["session"],
                    "provider": start["provider"], "source": source,
                    "start": start["ts"], "end": end["ts"], "window_seconds": seconds,
                    "start_bytes": first, "end_bytes": last, "tokens": tokens,
                    "model_observed": [start.get("model_observed"), end.get("model_observed")],
                    "effort_observed": [start.get("effort_observed"), end.get("effort_observed")]}
        intervals.append(interval)
        role = roles.setdefault(start["role"], {"intervals": 0, "window_seconds_sum": 0.0,
                                               "tokens": {key: 0 for key in TOKENS}})
        role["intervals"] += 1
        role["window_seconds_sum"] += seconds
        for key in TOKENS:
            role["tokens"][key] += tokens[key]
    return {
        "coverage": "somente_fontes_registradas",
        "methodology": [
            "Tokens = totais finais menos iniciais; entrada exclui cache lido e criado.",
            "Janela observada inclui espera; sua soma pode incluir trabalho em paralelo.",
            "Subagentes só entram com pares próprios; isto não presume o total da equipe.",
            "Modelo e esforço são observações declaradas, sem atribuição de tokens por modelo.",
        ],
        "by_role": roles, "intervals": intervals,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    collect = commands.add_parser("snapshot", help="Emite snapshot em JSON para stdout")
    collect.add_argument("--provider", choices=PROVIDERS, required=True)
    for name in ("transcript", "role", "session"):
        collect.add_argument(f"--{name}", required=True)
    for name in ("model", "effort"):
        collect.add_argument(f"--{name}")
    compare = commands.add_parser("report", help="Compara pares início/fim das fontes registradas")
    compare.add_argument("--pair", nargs=2, action="append", required=True,
                         metavar=("INICIO_JSON", "FIM_JSON"))
    args = vars(parser.parse_args(argv))
    command = args.pop("command")
    try:
        if command == "snapshot":
            result = snapshot(**args)
        else:
            pairs = [tuple(json.loads(Path(p).read_text(encoding="utf-8")) for p in pair)
                     for pair in args["pair"]]
            result = report(pairs)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except (OSError, ValueError, TypeError) as error:
        print(f"Consumo não medido: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
