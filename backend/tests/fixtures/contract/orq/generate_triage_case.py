"""Captura o tratamento de caixa pelo decisor Python de referência."""
import importlib.util
import json
import os
from pathlib import Path

reference = os.environ.get("HANGAR_ORQ_REFERENCE_ROOT")
if not reference:
    raise SystemExit("informe HANGAR_ORQ_REFERENCE_ROOT com o checkout Python de origem")
source = Path(reference) / "skills/orquestrar/scripts/orq_triage.py"
spec = importlib.util.spec_from_file_location("reference_triage", source)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
texts = [
    "ack; aguardo revisor que analise",
    "ack; aguardo revisor QUE analise",
    "ack; aguardo REVISOR Que analise",
    "ACK; AGUARDO REVISOR QUE analise",
    "ack; espero Revisor qUe analise",
    "ack; aguardando revisor Que analise",
    "ack; aguardo revisor",
    "ack; aguardo REVISOR",
    "ack; aguardo PARECER",
    "ack; aguardo VEREDITO",
    "ack; aguardo revisor. QUE analise",
    "ack; aguardo revisor\nQUE analise",
    "ack; aguardo revisor  QUE analise",
    "ack; aguardo revisor queijo",
]
Path(__file__).with_name("triage-case.json").write_text(json.dumps({
    "source_commit": "a0860f26b6e3f611806e780acf87588f6065b328",
    "cases": [{"text": text, "expected": module.decide(text)} for text in texts],
}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
