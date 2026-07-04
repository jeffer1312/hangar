from app.state import rate_limit_reset

# Banners plausiveis de rate-limit (texto EXATO nao documentado -- ver _LIMIT_RE em app/state.py,
# calibration knob). Cobrimos as variantes de frase + formato de horario descritas no spec.
LIMIT_PANES = {
    "Claude usage limit reached. Your limit will reset at 3pm.\n": "3pm",
    "5-hour limit reached · resets 15:30\n": "15:30",
    "You've hit your rate limit reached, try again at 9:05am\n": "9:05am",
}

# Sessao normal (sem banner) NAO pode disparar rate-limit (senao toda sessao vira "limitada").
NORMAL_PANES = [
    "✻ Elucidating… (3s · ↑ 1.2k tokens)\n",
    "● Done.\n⎿ Read file.py\n",
    "> ask me something\n────────────────\n  ~/proj  main  claude-opus",
    "",
]


def test_limit_panes_return_reset_time():
    for pane, expected in LIMIT_PANES.items():
        assert rate_limit_reset(pane) == expected, pane


def test_normal_panes_return_none():
    for pane in NORMAL_PANES:
        assert rate_limit_reset(pane) is None, pane
