from pathlib import Path

import pytest

from tests.omp_runtime import run_omp_driver


DRIVER = Path(__file__).resolve().parents[2] / "scripts/tests/claude-bridge-driver.ts"


@pytest.mark.parametrize("scenario", ["discovery", "conversion", "ownership", "memory", "disabled", "context", "legacy"])
def test_bridge_no_omp_real(tmp_path, scenario):
    run_omp_driver(DRIVER, tmp_path / scenario, {"BRIDGE_SCENARIO": scenario})


def test_runtime_recusa_extensao_que_nao_conclui(tmp_path):
    driver = tmp_path / "broken.ts"
    driver.write_text('export default function () { throw new Error("fixture-load-failure"); }', encoding="utf-8")
    with pytest.raises(AssertionError, match="Driver não concluiu|OMP falhou"):
        run_omp_driver(driver, tmp_path / "home")
