"""Parser do rodapé do modo de permissão — puro, sem tmux real."""
import app.permission_mode as pm


def _pane(linha):
    # helper: pane com linhas dummy + linha do rodapé no fim
    return f"alguma conversa\n{linha}\n❯ "


def test_parse_plan():
    assert pm.parse_permission_mode(_pane("⏸ plan mode on (shift+tab to cycle) · ← for agents")) == "plan"


def test_parse_auto():
    assert pm.parse_permission_mode(_pane("⏵⏵ auto mode on (shift+tab to cycle) · ← for agents")) == "auto"


def test_parse_manual():
    assert pm.parse_permission_mode(_pane("⏸ manual mode on · ← for agents")) == "manual"


def test_parse_accept_edits():
    assert pm.parse_permission_mode(_pane("⏵⏵ accept edits on (shift+tab to cycle) · ← for agents")) == "acceptEdits"


def test_parse_bypass():
    assert pm.parse_permission_mode(_pane("⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents")) == "bypassPermissions"


def test_parse_dont_ask():
    assert pm.parse_permission_mode(_pane("⏵⏵ don't ask on (shift+tab to cycle) · ← for agents")) == "dontAsk"


def test_parse_dont_ask_curvo():
    # apóstrofe tipográfico ’
    assert pm.parse_permission_mode(_pane("⏵⏵ don’t ask on (shift+tab to cycle) · ← for agents")) == "dontAsk"


def test_parse_ultima_linha_vence():
    pane = "⏸ plan mode on\n⏵⏵ auto mode on (shift+tab to cycle)\n⏸ manual mode on\n"
    # última linha com modo é manual
    assert pm.parse_permission_mode(pane) == "manual"


def test_parse_sem_glifo_cai_no_fallback():
    # pane estreito que cortou o glifo: segunda passada pega sem glifo
    pane = "alguma conversa\nplan mode on (shift+tab to cycle)\n❯ "
    assert pm.parse_permission_mode(pane) == "plan"


def test_parse_sem_modo_retorna_none():
    assert pm.parse_permission_mode("conversa sem rodapé\n❯ ") is None


def test_parse_conversa_citando_modo_com_glifo_falso_positivo_evitado():
    # conversa que cita \"auto mode on\" sem glifo não deve casar na primeira passada,
    # mas a segunda passada casaria (fallback). O comportamento atual é casar no fallback,
    # mas a primeira passada com glifo protege o caso comum. Testa que com glifo vazio,
    # o fallback ainda acha (para pane estreito).
    pane = "user: auto mode on is great\n⏸ plan mode on\n"
    assert pm.parse_permission_mode(pane) == "plan"


def test_parse_case_insensitive():
    assert pm.parse_permission_mode(_pane("⏸ PLAN MODE ON")) == "plan"
    assert pm.parse_permission_mode(_pane("⏵⏵ AUTO MODE ON")) == "auto"
