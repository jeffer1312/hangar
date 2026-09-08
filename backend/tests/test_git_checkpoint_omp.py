from pathlib import Path

import pytest

from tests.omp_runtime import run_omp_driver


DRIVER = Path(__file__).resolve().parents[2] / "scripts/tests/git-checkpoint-driver.ts"


@pytest.mark.parametrize("scenario", [
    "capture_restore", "resume_fork", "before_branch", "branch_scope", "late_capture",
    "subagent", "invalid_reference", "selection_and_partial", "legacy_pi", "directory_replaced", "native_timeout", "capture_deadline",
])
def test_checkpoint_no_omp_com_git_real(tmp_path, scenario):
    run_omp_driver(DRIVER, tmp_path / scenario, {"CHECKPOINT_SCENARIO": scenario})
