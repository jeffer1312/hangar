"""Congela o contrato executando a implementação Python antes de sua remoção."""
import json
import subprocess
import tempfile
import os
import unicodedata
from pathlib import Path

from app import engines, engine_probe

ROOT = Path(__file__).resolve().parents[5]
BASE = {"base_url": "https://provider.invalid/v1/", "api_key": "fixture-key", "model": "modelo"}
cases = []


def capture(name, body):
    try:
        expected = {"ok": engines._normalizar(name, body)}
    except (ValueError, OverflowError) as exc:
        expected = {"error": str(exc)}
    cases.append({"name": name, "body": body, "expected": expected})


capture("motor", BASE)
for name in ["", "A", "a b", "a/b", "-a", "a" * 32, "a" * 33, "a_b-9"]:
    capture(name, BASE)
for field in engines._CAMPOS:
    for value in [None, "", "  ", True, False, 0, -1, 1, 12.9, "42", "2.5", [], {}, "a\nb", " a "]:
        capture("motor", {**BASE, field: value})
for url in ["http://localhost:8317/v1", "http://127.0.0.1", "http://10.0.0.2", "http://192.168.0.2", "http://172.16.0.2", "http://[::1]", "https://host.invalid/path/v1///", "http://host.invalid", "ftp://host.invalid", "host.invalid", "https://"]:
    capture("motor", {**BASE, "base_url": url})
for value in ["1_000", "_12", "12_", "1__2", "+1_2", "-_1", "١٢", "１２", "1٢", "¹²"]:
    capture("motor", {**BASE, "context_window": value})
for zero in (n for n in range(0x110000) if unicodedata.decimal(chr(n), -1) == 0):
    for value in (chr(zero + 1) + chr(zero + 2), chr(zero + 1) + "_" + chr(zero + 2)):
        capture("motor", {**BASE, "context_window": value})
capture("motor", {**BASE, "extra": "ignorado", "context_window": 10**40})
output = {"source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "command": "PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate.py", "cases": cases}
Path(__file__).with_name("normalization.json").write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

env_cases = []
with tempfile.TemporaryDirectory() as root:
    os.environ["CP_ENGINES_FILE"] = str(Path(root) / "engines.json")
    for field in [None, *engines._CAMPOS]:
        for value in ([None] if field is None else [None, True, False, "", "500000", 42, "x\ny"]):
            record = {**BASE, "base_url": "https://provider.invalid"}
            if field is not None:
                record[field] = value
            Path(os.environ["CP_ENGINES_FILE"]).write_text(json.dumps({"motor": record}), encoding="utf-8")
            for selected in [None, "gpt-5[1m]", "outra"]:
                try:
                    expected = {"ok": engines.env_de("motor", selected)}
                except (ValueError, TypeError, KeyError) as exc:
                    expected = {"error": str(exc), "type": type(exc).__name__}
                env_cases.append({"record": record, "selected": selected, "expected": expected})
output["cases"] = env_cases
Path(__file__).with_name("environment.json").write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
probe_cases = []
for data in [None, {}, [], "x", [None, 1, {}, {"id": ""}, {"id": 1}, {"id": "m"}],
             [{"id": "m", "context_length": 0, "context_window": 123, "supports_image_in": False}],
             [{"id": "m", "supports_image_in": None, "capabilities": {"vision": True}}],
             [{"id": "m", "capabilities": {"image_in": True}}]]:
    engine_probe._buscar = lambda *_: {"data": data}
    try:
        expected = {"ok": engine_probe.listar_modelos("https://provider.invalid", "fixture-key")}
    except RuntimeError as exc:
        expected = {"error": str(exc)}
    probe_cases.append({"data": data, "expected": expected})
output["cases"] = probe_cases
Path(__file__).with_name("models.json").write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
