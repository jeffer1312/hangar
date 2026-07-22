"""Testes de with_cwd_real: cwd_real resolvido (realpath) pro casamento sessão↔projeto."""
import importlib.machinery
import importlib.util
import os
from pathlib import Path

# cp-panel-data não tem extensão .py — mesmo carregamento de test_scan_folder.py.
_path = Path(__file__).with_name("cp-panel-data")
spec = importlib.util.spec_from_file_location(
    "cp_panel_data", _path,
    loader=importlib.machinery.SourceFileLoader("cp_panel_data", str(_path)))
cpd = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cpd)


def test_resolve_symlink(tmp_path):
    real = tmp_path / "real"
    real.mkdir()
    link = tmp_path / "link"
    link.symlink_to(real)
    d = cpd.with_cwd_real({"cwd": str(link)})
    assert d["cwd_real"] == os.path.realpath(str(real))


def test_colapsa_dotdot(tmp_path):
    (tmp_path / "a").mkdir()
    d = cpd.with_cwd_real({"cwd": str(tmp_path / "a" / ".." / "a")})
    assert d["cwd_real"] == os.path.realpath(str(tmp_path / "a"))


def test_cwd_vazio_ou_ausente():
    assert cpd.with_cwd_real({})["cwd_real"] == ""
    assert cpd.with_cwd_real({"cwd": ""})["cwd_real"] == ""


def test_slim_inclui_cwd_real(tmp_path):
    out = cpd.slim({"name": "x", "cwd": str(tmp_path)}, None)
    assert out["cwd_real"] == os.path.realpath(str(tmp_path))
