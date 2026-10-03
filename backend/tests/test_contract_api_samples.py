# backend/tests/test_contract_api_samples.py
"""As amostras que o crate hangar-api testa saem dos modelos atuais: mudou models.py, regenere."""
import importlib.util
from pathlib import Path

GEN = Path(__file__).parent / "fixtures" / "contract" / "gen_api_samples.py"


def _gen():
    spec = importlib.util.spec_from_file_location("gen_api_samples", GEN)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_api_samples_match_the_models():
    gen = _gen()
    have = {p.name: p.read_text(encoding="utf-8") for p in gen.OUT.glob("*.json")}
    assert have == gen.samples(), "rode: uv run python tests/fixtures/contract/gen_api_samples.py"
