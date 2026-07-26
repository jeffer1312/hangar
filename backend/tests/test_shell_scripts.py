"""Roda os scripts de shell que travam contrato mas não tinham entrypoint nenhum no `pytest -q`.

`test-wrappers.sh` é a ÚNICA cobertura de uma regressão que já foi ao ar (claude-engine <motor> -c
dropava o motor porque `pre` só era montado DEPOIS do early-return dessas flags — achado só testando
à mão). Sem isto no `pytest`, nada impede o mesmo bug de voltar num refactor futuro do wrapper.
"""
import shutil
import subprocess
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]


def test_wrappers_sh():
    if not shutil.which("zsh") or not shutil.which("fish"):
        pytest.skip("precisa de zsh e fish no PATH")
    # timeout: hoje os dois scripts rodam em ~2s, sem rede e sem tmux. Mas um deles ganhar uma espera
    # sem fim (prompt interativo, laço de retry) penduraria o `pytest -q` inteiro sem recuperação —
    # justo o que este arquivo existe pra evitar. Estourar é falha, igual exit != 0.
    r = subprocess.run([str(REPO / "scripts" / "test-wrappers.sh")],
                        cwd=REPO, capture_output=True, text=True, timeout=60)
    assert r.returncode == 0, r.stdout + r.stderr


def test_statusline_sh():
    if not shutil.which("node"):
        pytest.skip("precisa de node no PATH")
    r = subprocess.run([str(REPO / "scripts" / "test-statusline.sh")],
                        cwd=REPO, capture_output=True, text=True, timeout=60)
    assert r.returncode == 0, r.stdout + r.stderr
