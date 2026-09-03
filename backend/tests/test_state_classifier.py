from pathlib import Path
from unittest.mock import patch

import pytest

from app import state as state_mod
from app.state import classify, StateMonitor


def test_working_with_spinner_label():
    state, label, q, opts = classify("● PONG\n\n✽ Elucidating…\n\n❯ \n  ← for agents\n")
    assert state == "working"
    assert label == "Elucidating…"


def test_working_elapsed_form():
    state, label, q, opts = classify("✻ Crunched for 8s\n❯ \n")
    assert state == "working" and label == "Crunched for 8s"


def test_assistant_bullet_is_not_spinner():
    # ● is the message bullet, not a spinner glyph
    state, label, q, opts = classify("● PONG\n❯ \n")
    assert state == "idle"


def test_awaiting_input_parses_question_and_options():
    pane = (
        "   Claude has written up a plan. Would you like to proceed?\n"
        "\n"
        "   ❯ 1. Yes, and bypass permissions\n"
        "     2. Yes, manually approve edits\n"
        "     3. No, keep planning\n"
    )
    state, label, question, options = classify(pane)
    assert state == "awaiting_input"
    assert question == "Claude has written up a plan. Would you like to proceed?"
    assert options == ["Yes, and bypass permissions", "Yes, manually approve edits", "No, keep planning"]


def test_awaiting_input_option_cut_at_preview_box():
    # AskUserQuestion com `preview`: box (│...│) renderiza NA MESMA LINHA da opção. O label deve
    # parar na borda │ — sem o corte, o conteúdo do preview poluia a opção (bug real observado numa sessão).
    pane = (
        "   Como deixo o meu?\n"
        "\n"
        " ❯ 1. System no topo (igual aos     ╭──────────────────────────────╮\n"
        "      irmãos)                        │ using System.Reflection;     │\n"
        "   2. Alfabético (obedece           │ using Xunit;                 │\n"
        "      .editorconfig)                 ╰──────────────────────────────╯\n"
    )
    state, label, question, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["System no topo (igual aos", "Alfabético (obedece"]


def test_awaiting_input_option_cut_at_square_corner_preview_box():
    """Mesmo corte, mas com os cantos RETOS (┌└─) em vez dos arredondados (╭╰).

    O teste acima só exercitava ╭╮╰╯, e a regex só tinha esses — o box do preview desenhado com
    canto reto passava batido: a opção que cai na linha da borda de cima vinha
    "Escolher dimensão +          ┌────────────", nunca casava com o label do sidecar no gate do
    sse (_ask_question_event), e a pergunta degradava pro OptionButtons — perdia descrição E
    preview no app, com essa opção aparecendo VAZIA. Print do usuário em 06/08/2026.
    """
    pane = (
        "Como deve ser a seção de comparação de gasto na tela de Custos?\n"
        "\n"
        " ❯ 1. Escolher dimensão +          ┌──────────────────────────────\n"
        "      valores                      │ Comparar por: [provedor ▾]\n"
        "   2. A vs B com filtro            │ Marcados: [x] conta Anthropic\n"
        "      completo                     │\n"
        "   3. Tabela cruzada (fonte ×      │  ┌ conta Anthropic ─┐\n"
        "      provedor)                    └──────────────────────────────\n"
    )
    state, _label, _question, options = classify(pane)
    assert state == "awaiting_input"
    assert options == [
        "Escolher dimensão +",
        "A vs B com filtro",
        "Tabela cruzada (fonte ×",
    ]


def test_box_char_inside_the_label_itself_survives():
    """`─` é caractere de box, mas pode aparecer no PRÓPRIO texto da opção.

    O corte exige 2+ espaços antes da borda (o box do preview mora numa coluna à direita, então há
    sempre um vão). Sem isso, "Rodar tudo ─ inclusive os lentos" virava "Rodar tudo" e o casamento
    por prefixo no sse aprovava um label errado — resposta na opção errada, calado.
    """
    pane = (
        "O que rodar?\n"
        "\n"
        " ❯ 1. Rodar tudo ─ inclusive os lentos\n"
        "   2. Sim — e não perguntar de novo\n"
    )
    _state, _label, _question, options = classify(pane)
    assert options == ["Rodar tudo ─ inclusive os lentos", "Sim — e não perguntar de novo"]


def test_numbered_list_without_cursor_stays_idle():
    # a plain numbered list (no ❯ cursor on an option) is NOT a widget
    state, *_ = classify("Steps:\n  1. do this\n  2. do that\n❯ \n")
    assert state == "idle"


def test_idle_when_no_spinner_or_widget():
    state, label, q, opts = classify("❯ \n  ← for agents\n")
    assert state == "idle"


def test_real_fixtures():
    fx = Path(__file__).parent / "fixtures"
    assert classify((fx / "pane_idle.txt").read_text(encoding="utf-8"))[0] == "idle"
    s, lbl, *_ = classify((fx / "pane_thinking.txt").read_text(encoding="utf-8"))
    assert s == "working" and lbl == "Elucidating…"
    s2, _, q2, opts2 = classify((fx / "pane_awaiting_input.txt").read_text(encoding="utf-8"))
    assert s2 == "awaiting_input" and opts2 and "proceed?" in (q2 or "")


def test_quoted_menu_in_scrollback_is_idle():
    """O assistente citou o menu nativo na propria mensagem ("❯ 1. Yes, switch to xhigh / 2. No,
    go back"). Esse "❯ N." vive no scrollback com o composer vivo (input box) renderizado ABAIXO,
    entao NAO e um widget selecionavel -> idle, nao awaiting_input (senao o app trava num menu
    fantasma). Captura real do pane que travou o hangar."""
    fx = Path(__file__).parent / "fixtures" / "pane_quoted_menu_scrollback.txt"
    state, _, _, options = classify(fx.read_text(encoding="utf-8"))
    assert state == "idle", f"menu citado virou {state} com opcoes {options}"


def test_askuserquestion_real_fixture():
    """A AskUserQuestion (widget do assistente) capturada de verdade do pane: o classificador
    tem que extrair a pergunta e as opcoes reais, escopadas ao box do picker."""
    fx = Path(__file__).parent / "fixtures" / "pane_askuserquestion.txt"
    state, _, question, options = classify(fx.read_text(encoding="utf-8"))
    assert state == "awaiting_input"
    assert "Captura de formato" in (question or "")
    assert "Opção Alpha" in options
    assert "Opção Bravo" in options


def test_picker_options_exclude_scrollback_numbered_lines():
    """Bug real: o classificador coletava TODA linha numerada do pane. Uma lista numerada no
    scrollback (acima de um bullet) NAO pode vazar pras opcoes do picker."""
    pane = (
        "● Earlier I listed steps:\n"
        "  1. first scrollback item\n"
        "  2. second scrollback item\n"
        "\n"
        "● Now pick one:\n"
        "   ❯ 1. Real Alpha\n"
        "     2. Real Bravo\n"
        "Enter to select · ↑/↓ to navigate · Esc to cancel\n"
    )
    state, _, _, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["Real Alpha", "Real Bravo"]
    assert all("scrollback" not in o for o in options)


def test_chip_header_excludes_prose_numbered_list_above():
    """Bug real (web mostrava 10 opcoes): uma AskUserQuestion logo abaixo de uma LISTA NUMERADA
    EM PROSA, sem bullet ● entre elas. O chip ☐ do widget e o topo do bloco; os '1. ... 2. ...'
    da prosa ficam acima do chip e NAO podem virar opcoes falsas."""
    pane = (
        "● Caminho único e limpo:\n"
        "  1. Aqui digita /exit\n"
        "  2. No shell: tmux kill-server\n"
        "  3. Abre tmux limpo\n"
        "  4. Retoma esta conversa\n"
        "\n"
        "☐ Status bar\n"
        "Status bar do tmux: religar pra ver sessão/janelas?\n"
        "\n"
        "❯ 1. Religar minimal\n"
        "  2. Deixar off\n"
        "Enter to select · ↑/↓ to navigate · Esc to cancel\n"
    )
    state, _, question, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["Religar minimal", "Deixar off"]
    assert "religar" in (question or "").lower()


@pytest.mark.asyncio
async def test_monitor_emits_only_on_change():
    # Dedup: o spinner byte-identico no 2o poll NAO re-emite. Vai do spinner pro MENU (awaiting_input
    # e autoritativo/sem debounce) em vez de voltar pra idle: idle->working tem IDLE_DEBOUNCE e o
    # status_line flipa ao perder o spinner (re-emite working), o que polui um teste de "so na mudanca".
    panes = iter([
        "❯ \n",                  # idle
        "✽ Elucidating…\n",      # working
        "✽ Elucidating…\n",      # byte-identico -> NAO emite de novo
        "❯ 1. Religar\n  2. Deixar off\nEnter to select · ↑/↓ to navigate · Esc to cancel\n",  # menu
    ])
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=lambda *a, **k: next(panes)):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if len(seen) == 3:
                break
    # 3 emits, nao 4: o 2o spinner identico foi suprimido (senao haveria um working extra no meio).
    assert seen == [("idle", None), ("working", "Elucidating…"), ("awaiting_input", None)]


@pytest.mark.asyncio
async def test_monitor_frozen_completed_marker_reads_idle():
    """Regression (bug #4): a completed-turn marker ("✻ Worked for 13s") lingers in the pane
    while Claude is idle. It is shaped exactly like a live spinner; a single frame can't tell
    them apart (classify reports 'working' for any spinner). The proof it's frozen is that it
    NEVER changes — so the monitor reports 'working' briefly, then DOWNGRADES to 'idle' after
    STALE_LIMIT identical polls. Net result the UI cares about: it does NOT stay stuck working."""
    frozen = "● the answer\n✻ Worked for 13s\n────\n❯ \n────\n  ⏵⏵ bypass permissions on\n"
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", return_value=frozen):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if ev.state == "idle":
                break
    assert seen[-1] == ("idle", None)                 # converge pra idle (composer nao trava working)
    assert ("working", "Worked for 13s") in seen      # flash breve antes do downgrade (tradeoff anti-flicker)


@pytest.mark.asyncio
async def test_monitor_animating_spinner_reads_working():
    """A live spinner animates (glyph cycles) while its label holds — that change is the
    proof of life that distinguishes it from a frozen marker."""
    panes = iter([
        "❯ \n",                       # idle baseline
        "✽ Pondering…\n❯ \n",         # spinner appears
        "✶ Pondering…\n❯ \n",         # glyph cycled -> alive (same label)
        "❯ \n",                       # idle again
    ])
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=lambda *a, **k: next(panes)):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if ev.state == "working":
                break
    assert seen[0] == ("idle", None)
    assert seen[-1] == ("working", "Pondering…")


@pytest.mark.asyncio
async def test_monitor_carries_loop_fields(tmp_path, monkeypatch):
    # Chip 🔁 no Chat mobile vem do evento 'state' por sessao (sem reter o sessionsStore -> 1 SSE/sessao).
    from app import loop as loop_mod
    monkeypatch.setattr(loop_mod.settings, "projects_dir", tmp_path / "projects")
    d = loop_mod.new_loop("g", "pytest", 7, True)
    d["iter"] = 3
    loop_mod.LoopLink("cc").set(d)
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", return_value="❯ \n"):
        mon = StateMonitor("cc", poll=0.001)
        first = None
        async for ev in mon.stream():
            first = ev
            break
    assert first is not None
    assert first.loop_status == "running" and first.loop_iter == 3 and first.loop_max == 7


async def _run_until(mon, polls: int, pane: str):
    """Roda o monitor por `polls` capturas e devolve os eventos emitidos (o stream so emite na
    mudanca). Encerra estourando do proprio capture_pane — deterministico, sem sleep."""
    n = {"i": 0}

    def frame(*a, **k):
        n["i"] += 1
        if n["i"] > polls:
            raise RuntimeError("fim do teste")
        return pane

    seen = []
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=frame), \
         patch.object(state_mod.hook_state, "get_state", return_value=("working", 1.0)):
        with pytest.raises(RuntimeError):
            async for ev in mon.stream():
                seen.append((ev.state, ev.label))
    return seen


@pytest.mark.asyncio
async def test_hook_working_marker_expires_for_claude():
    # Comportamento de HOJE, travado: marcador "working" preso (claude morto mid-turn) deixa de ser
    # honrado apos HOOK_WORKING_GRACE polls sem spinner -> o pane volta a mandar.
    mon = StateMonitor("cc", poll=0.001, sid_get=lambda: "sid")
    seen = await _run_until(mon, polls=StateMonitor.HOOK_WORKING_GRACE + 3, pane="❯ \n")
    assert seen[0] == ("working", None)
    assert ("idle", None) in seen


@pytest.mark.asyncio
async def test_hook_working_marker_never_expires_without_a_readable_spinner():
    # FINDING 2: SPINNER_GLYPHS sao os do Claude; o loader do Pi e braille (⠋⠙⠹…), entao o contador
    # `no_spinner` sobe DURANTE o turno e o marcador working era descartado no meio da conversa
    # (chat mostrando "ocioso" com o agente trabalhando). Sem spinner legivel o marcador e a UNICA
    # verdade — mesma politica que a lista ja usa (registry.py:719, sem grace nenhuma).
    mon = StateMonitor("pi", poll=0.001, sid_get=lambda: "sid", hook_grace=None)
    seen = await _run_until(mon, polls=StateMonitor.HOOK_WORKING_GRACE + 8,
                            pane="⠙ pensando\n❯ \n")
    assert seen == [("working", None)], f"caiu pra idle no meio do turno: {seen}"


def test_is_overlay_true_with_nav_footer():
    pane = "alguma conversa\n● resposta\n────────\n  Esc to cancel · Enter to select\n"
    assert state_mod.is_overlay(pane) is True


def test_is_overlay_false_without_footer():
    assert state_mod.is_overlay("● PONG\n❯ \n") is False


def test_status_line_is_the_chrome_below_the_input_box():
    pane = (
        "● the answer\n"
        "✻ Worked for 1s\n"
        "──────────────────────────────\n"
        "❯ \n"
        "──────────────────────────────\n"
        "  📂 proj │ 💬 43k/606 40k/1M │ 💵 $0.47 │ 🕐 14:00\n"
        "  ⏵⏵ bypass permissions on · ← for agents\n"
    )
    sl = state_mod.status_line(pane)
    assert "💬 43k/606" in sl and "💵 $0.47" in sl
    assert "the answer" not in sl   # conversation excluded
    assert "✻ Worked" not in sl     # spinner / completed marker excluded


# --- Pi: a ancora e a caixa arredondada do composer, nao a regua reta do Claude ----------------
# pane_pi_idle.txt e um `tmux capture-pane -p` REAL de uma sessao Pi ociosa (Pi 0.82.1). Sem a
# ancora da caixa, o fallback devolvia as 2 ultimas linhas nao-vazias -> a borda `╰───╯` colada no
# chip, ou conversa pura quando o usuario nao tem extensao de statusline.

def _pane_pi() -> str:
    return (Path(__file__).parent / "fixtures" / "pane_pi_idle.txt").read_text(encoding="utf-8")


def test_status_line_pi_is_the_row_below_the_composer_box():
    sl = state_mod.status_line(_pane_pi())
    assert sl is not None
    assert sl.startswith("🤖 kimi-for-coding (high)") and "💬 sessão 135in/470out" in sl
    assert "╰" not in sl and "╭" not in sl      # borda da caixa fora
    assert "Como posso te ajudar" not in sl     # conversa fora


def test_status_line_pi_sem_extensao_de_statusline_e_none():
    # MESMO pane sem a ultima linha: e exatamente o que ve quem nao instalou a extensao de
    # statusline. Nada abaixo da caixa -> nada. Devolver conversa seria pior que devolver vazio.
    pane = "\n".join(_pane_pi().splitlines()[:-1]) + "\n"
    assert state_mod.status_line(pane) is None


def test_status_line_claude_fixtures_byte_identical():
    # Trava de nao-regressao: a ancora nova NAO pode mover um byte do que o Claude ja devolvia.
    # Valores conferidos contra a implementacao anterior (so a regua reta) nestes mesmos arquivos.
    fx = Path(__file__).parent / "fixtures"
    esperado = {
        "pane_idle.txt": (
            "  Opus 4.8 (1M context) ~high 📁 claude-pocket master ⊙6m ↑0↓0 │  🧠4.9G\n"
            "  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents"
        ),
        "pane_thinking.txt": (
            "  Opus 4.8 (1M context) ~high 📁 claude-pocket master ⊙6m ↑0↓0 │  🧠4.9G\n"
            "  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents"
        ),
    }
    for nome, valor in esperado.items():
        assert state_mod.status_line((fx / nome).read_text(encoding="utf-8")) == valor, nome
    # O banner de boas-vindas do Claude TEM `╰───╯` (linha 12 do pane_idle) — a asserção acima já
    # prova que ele nao vira ancora: a regua do input, mais abaixo, continua ganhando.


def test_pi_question_picker_is_awaiting_input():
    # Picker do Pi (tool `question`): cursor ascii "> N." + rodape de navegacao no FUNDO do pane.
    # Medido no pi 0.82.1 (a pergunta "Quando mostrar o diff" desta feature).
    pane = (
        "● Question No pi o diff do Edit aparece aberto na conversa?\n"
        + ("─" * 60) + "\n"
        "\n"
        "[Quando mostrar o diff]\n"
        "No pi o diff do Edit aparece aberto na conversa?\n"
        "\n"
        "> 1. Sempre aberto na conversa\n"
        "  2. Só ao expandir (toque/clique)\n"
        "  3. Aberto por padrão, recolhível\n"
        "  4. Type something.\n"
        "↑↓ navigate • Enter to select • Esc to cancel\n"
        + ("─" * 60) + "\n"
        " k3 (high) | hangar | sessão 45k\n"
    )
    state, label, question, options = classify(pane)
    assert state == "awaiting_input"
    assert question == "No pi o diff do Edit aparece aberto na conversa?"
    assert options == ["Sempre aberto na conversa", "Só ao expandir (toque/clique)",
                       "Aberto por padrão, recolhível", "Type something."]


def test_rascunho_numerado_no_composer_nao_e_menu():
    """O composer com um rascunho que comeca em numero NAO e uma pergunta.

    Visto ao vivo em 25/08/2026: o usuario digitou "1. sim pode editar 2. mostra a data com rotulo
    neutro" e deixou parado no composer. O Claude Code desenha `❯ ` na frente do que se digita, entao
    a linha ficou identica a um cursor de opcao — o app anunciou "aguardando sua resposta" com UMA
    opcao falsa e o marcador de fim de turno no lugar da pergunta, com a sessao ociosa.

    A guarda de "prosa citada" nao alcanca este caso: ela procura um composer ABAIXO do cursor, e
    aqui o cursor E o composer.
    """
    pane = (
        "  Está tudo andando: revisor no portão da folha.\n"
        "\n"
        "* Baked for 1m 52s\n"
        "\n"
        "❯ 1. sim pode editar 2. mostra a data com rótulo neutro\n"
        "  🤖 Opus5·1M (high+) │ 📁 meu-projeto\n"
    )
    state, _, _, options = classify(pane)
    assert state == "idle"
    assert options is None


def test_rascunho_numerado_no_composer_do_pi_nao_e_menu():
    # Mesma coisa no cursor do Pi ("> N."), que desenha o rodape de navegacao por outro motivo.
    pane = (
        "Resposta anterior do agente.\n"
        "> 1. pode subir 2. depois me avisa\n"
        "↑↓ navigate • Enter to select • Esc to cancel\n"
    )
    state, *_ = classify(pane)
    assert state == "idle"


def test_opcao_com_numero_no_texto_continua_sendo_menu():
    """Contra-exemplo da guarda acima: "2.1" dentro de uma opcao legitima nao pode matar o menu.

    Recusar aqui e PIOR que o bug que a guarda conserta — vira sessao que pede resposta e nao avisa
    ninguem. Por isso o marcador embutido exige espaco dos dois lados do numero."""
    pane = (
        "Qual versão instalar?\n"
        "\n"
        "❯ 1. Instalar a versão 2.1 do pacote\n"
        "  2. Manter a atual\n"
        "  3. Cancelar\n"
    )
    state, _, question, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["Instalar a versão 2.1 do pacote", "Manter a atual", "Cancelar"]


def test_preview_com_lista_numerada_nao_mata_o_menu():
    """A AskUserQuestion com `preview` desenha o box NA MESMA LINHA da opcao, e o que ele mostra
    costuma ser arquivo (README, changelog) com lista numerada dentro.

    Achado de revisao: a guarda do rascunho, olhando a linha INTEIRA, lia o "1. instale 2. configure"
    do PREVIEW como lista corrida digitada e recusava o menu — o defeito inverso, e o pior dos dois
    (sessao parada esperando resposta com o app calado). Ela vale so no label, antes da borda do box.
    """
    pane = (
        "Qual README usar de exemplo?\n"
        "\n"
        "❯ 1. Padrao          │ Passos: 1. instale  2. configure  │\n"
        "  2. Minimo          │ (nada)                            │\n"
    )
    state, _, _, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["Padrao", "Minimo"]


def test_numero_colado_no_texto_nao_e_marcador():
    # O marcador embutido exige espaco dos DOIS lados: "v2. Confirmar" e texto da opcao, nao um
    # segundo item de lista. Sem a exigencia do espaco antes, este menu legitimo morreria.
    pane = (
        "Qual caminho?\n"
        "\n"
        "❯ 1. Migrar pra v2. Confirmar depois\n"
        "  2. Ficar na v1\n"
    )
    state, *_ = classify(pane)
    assert state == "awaiting_input"


def test_uma_opcao_sozinha_nao_e_menu():
    # Escolher entre uma coisa so nao e escolha: todo menu real medido tem de 3 a 5 opcoes, e uma
    # sozinha e sinal de bloco montado em cima de texto que apenas PARECE lista.
    pane = (
        "Texto qualquer da conversa.\n"
        "\n"
        "❯ 1. isto aqui e um rascunho\n"
    )
    state, *_ = classify(pane)
    assert state == "idle"


# Glifos de nerd font do picker do omp, por codigo pro arquivo ficar legivel (fonte comum nao
# desenha a area de uso privado): chevron da linha selecionada e circulo que marca toda opcao.
_OMP_SEL = chr(0xF054)
_OMP_OPT = chr(0xF10C)


def test_omp_ask_picker_is_awaiting_input():
    """Picker da tool `ask` do omp: opcoes SEM numero, marcadas por glifos de nerd font.

    Medido no omp 18.1.4: a linha selecionada traz U+F054 (chevron) antes do U+F10C (circulo) que
    marca toda opcao; o rodape e "Enter select · n note · ↑/↓ move · Esc cancel". A linha de texto
    livre ("Other (type your own)") e sempre a ultima.
    """
    pane = (
        "● Perguntando preferência de cor\n"
        "╭─ Ask " + "─" * 45 + "\n"
        "│ Qual cor você prefere?\n"
        "├" + "─" * 51 + "\n"
        f"│ {_OMP_SEL} {_OMP_OPT} Azul\n"
        f"│   {_OMP_OPT} Verde\n"
        f"│   {_OMP_OPT} Other (type your own)\n"
        "│\n"
        "├" + "─" * 51 + "\n"
        "│ Enter select · n note · ↑/↓ move · Esc cancel\n"
        "╰" + "─" * 51 + "\n"
        " k3 (high) | hangar | sessão 45k\n"
    )
    state, label, question, options = classify(pane)
    assert state == "awaiting_input"
    assert question == "Qual cor você prefere?"
    assert options == ["Azul", "Verde", "Other (type your own)"]


def test_omp_ask_picker_citado_sem_rodape_no_fundo_nao_e_menu():
    # Mesma trava do Pi: o picker do omp so vale com o rodape de navegacao no FUNDO do pane.
    pane = (
        "Veja como o omp desenha:\n"
        f"│ {_OMP_SEL} {_OMP_OPT} Azul\n"
        f"│   {_OMP_OPT} Verde\n"
        "│ Enter select · n note · ↑/↓ move · Esc cancel\n"
        + "".join(f"linha de conversa {i} depois da citacao\n" for i in range(12))
        + "╰" + "─" * 40 + "╯\n"
        + " k3 (high) | sessao\n"
    )
    state, *_ = classify(pane)
    assert state == "idle"


def test_pi_cursor_citation_without_live_footer_is_not_a_menu():
    # "> 1." CITADO em prosa (o rodape da citacao subiu no scrollback) nao e picker vivo — sem a
    # trava do rodape-no-fundo isto travava a sessao num menu fantasma.
    pane = (
        "Veja como o pi desenha:\n"
        "> 1. Sempre aberto na conversa\n"
        "  2. Só ao expandir\n"
        "↑↓ navigate • Enter to select • Esc to cancel\n"
        + "".join(f"linha de conversa {i} depois da citacao\n" for i in range(12))
        + "╰" + "─" * 40 + "╯\n"
        + " k3 (high) | sessao\n"
    )
    state, *_ = classify(pane)
    assert state == "idle"
