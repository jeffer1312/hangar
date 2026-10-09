"""O golden das políticas puras sai das funções Python: mudou a regra, regenere."""
import subprocess
import sys
from pathlib import Path

CONTRACT = Path(__file__).parent / "fixtures" / "contract"
NAMES = {"prepare_prompt.json", "format_status_claude.json", "format_status_codex.json", "skill_catalog.json",
         "last_usage.json", "reload_stamp.json", "unknown_private.json",
         "parked_state.json"}


def test_local_policy_golden_is_current(tmp_path):
    # Processo à parte: o gerador troca TZ, HOME e o relógio, e não pode vazar para os outros testes.
    code = "import sys; sys.path.insert(0, sys.argv[1]); import gen_local_policy; gen_local_policy.write(sys.argv[2])"
    subprocess.run([sys.executable, "-c", code, str(CONTRACT), str(tmp_path)], check=True, timeout=300,
                   cwd=Path(__file__).parents[1])
    assert {p.name for p in tmp_path.iterdir()} == NAMES
    for name in sorted(NAMES):
        have = (CONTRACT / "local_policy" / name).read_text(encoding="utf-8")
        assert (tmp_path / name).read_text(encoding="utf-8") == have, \
            f"{name} desatualizado: rode tests/fixtures/contract/gen_local_policy.py"
