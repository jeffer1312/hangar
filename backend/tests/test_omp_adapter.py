import pytest


def test_model_args_aceita_omp_com_thinking():
    from app import model_args
    assert model_args.args_de("omp", "x/y", "max") == ["--model", "x/y", "--thinking", "max"]
    with pytest.raises(ValueError):
        model_args.args_de("omp", None, "ultracode")


def test_cli_probe_e_stats_conhecem_omp():
    from app import cli_probe
    assert cli_probe._BIN["omp"] == "omp"


def test_argv0_omp_vira_provider_omp():
    from app.registry import _provider_do_argv
    assert _provider_do_argv(["omp"]) == "omp"
    assert _provider_do_argv(["pi"]) == "pi"
