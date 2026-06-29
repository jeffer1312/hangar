from app.state import is_login

# Painel real do welcome/login do Claude Code tem variantes; cobrimos os marcadores ancorados.
LOGIN_PANES = [
    "Open this URL to authenticate:\n  https://claude.ai/oauth/authorize?code=abc123&state=xyz\n",
    "Paste code here if prompted:\n  >\n",
    "Select login method\n  1. Claude account with subscription\n  2. Anthropic Console account\n",
    "Let's get started.\n\nChoose the text style that looks best with your terminal:\n  1. Dark mode\n",
]

# Sessao ja logada / em uso NAO pode disparar login (senao todo chat vira "precisa logar").
NON_LOGIN_PANES = [
    "✻ Elucidating… (3s · ↑ 1.2k tokens)\n",
    "● Done.\n⎿ Read file.py\n",
    "> ask me something\n────────────────\n  ~/proj  main  claude-opus",
    "",
]


def test_login_panes_detected():
    for pane in LOGIN_PANES:
        assert is_login(pane), pane


def test_normal_panes_not_login():
    for pane in NON_LOGIN_PANES:
        assert not is_login(pane), pane
