def test_argv0_omp_vira_provider_omp():
    from app.registry import _provider_do_argv
    assert _provider_do_argv(["omp"]) == "omp"
    assert _provider_do_argv(["pi"]) == "pi"
