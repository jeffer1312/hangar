"""orq_triage (skills/orquestrar/scripts/orq_triage.py): a regex que dispensa o árbitro sem Jev."""
import importlib.util
import json
from pathlib import Path

import pytest

pytestmark = pytest.mark.usefixtures("hangar_server_bin")

TRIAGE = Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq_triage.py"
# Conversa privada classificada à mão: fica fora do git, e o teste pula onde ela não existe.
LABELED = Path.home() / ".hangar" / "orq" / "jev-calibracao" / "regex" / "labeled.json"


@pytest.fixture(scope="module")
def rx():
    spec = importlib.util.spec_from_file_location("orq_triage", TRIAGE)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def test_nenhum_recado_que_precisava_do_arbitro_e_descartado(rx):
    if not LABELED.exists():
        pytest.skip(f"{LABELED} ausente (conversa privada, fora do git)")
    rows = json.loads(LABELED.read_text(encoding="utf-8"))
    wrong = [r["id"] for r in rows if r["label"] == "wake" and rx.decide(r["text"])[0] == "drop"]
    assert wrong == []


@pytest.mark.parametrize("text, expected", [
    ("janela de prova fechada", ("drop", "janela")),
    ("T4 rodada 2 entregue ao revisor", ("drop", "entrega")),
    ("APROVA T5 r2 enviado ao executor", ("drop", "veredito")),
    # Cada um abaixo casaria um status; o veto ou o tamanho acorda.
    ("[decisao] janela de prova fechada", ("wake", None)),
    ("janela de prova fechada. " + "x" * 700, ("wake", None)),
    ("Peço a tela", ("wake", None)),
    ("APROVA T5 r2 enviado ao executor. NOTED 1 é teu", ("wake", None)),
    # Sem status conhecido, acorda.
    ("ok", ("wake", None)),
])
def test_decide(rx, text, expected):
    assert rx.decide(text) == expected


def test_limite_de_tamanho(rx):
    assert rx.MAX_LEN == 600
    base = "janela de prova fechada"
    assert rx.decide(base + " " * (600 - len(base)))[0] == "drop"
    assert rx.decide(base + " " * (601 - len(base)))[0] == "wake"
