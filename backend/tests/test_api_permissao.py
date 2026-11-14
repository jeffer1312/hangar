"""Modo de permissão na criação — borda HTTP."""


def test_monitor_do_pane_preserva_ultima_escolha_antes_de_entrar_no_plano():
    from app import permission_mode

    permission_mode._ultimos_nao_plan.clear()
    assert permission_mode.observar_pane("sid", "⏵⏵ auto mode on") == "auto"
    assert permission_mode.observar_pane("sid", "⏸ plan mode on") == "plan"
    assert permission_mode.observar_modo("sid", "plan") == "auto"


def test_modos_intermediarios_de_operacao_controlada_nao_substituem_anterior():
    from app import permission_mode

    permission_mode._ultimos_nao_plan.clear()
    permission_mode.observar_pane("sid-controlado", "⏵⏵ auto mode on", sessao="sess")
    with permission_mode.operacao_controlada("sess"):
        permission_mode.observar_pane("sid-controlado", "⏸ manual mode on", sessao="sess")
        permission_mode.observar_pane("sid-controlado", "⏵⏵ bypass permissions on", sessao="sess")
        permission_mode.observar_pane("sid-controlado", "⏸ plan mode on", sessao="sess")

    assert permission_mode.observar_modo("sid-controlado", "plan") == "auto"
