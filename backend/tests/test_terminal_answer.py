from unittest.mock import patch
from app import terminal_input as ti


def test_send_prompt_waits_for_ready_before_sending():
    # Core bug: msg mandada com claude bootando era engolida. Com o gate de entregabilidade, o pane
    # precisa estar VIVO (has_session) + entregavel (sem overlay) + READY (rodape 'bypass permissions')
    # pra send_prompt enviar e devolver "sent". O wait-for-boot em si (rodape ausente -> espera/timeout)
    # fica coberto por test_wait_input_ready_times_out_then_false.
    ready = "❯ \n⏵⏵ bypass permissions on (shift+tab to cycle)"
    keys = []
    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: ready), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)):
        assert ti.TerminalInput().send_prompt("s", "oi") == "sent"
    assert keys == ["oi", "Enter"]  # enviou (pane entregavel + ready)


def test_wait_input_ready_times_out_then_false():
    with patch.object(ti, "_capture", lambda name: "bootando sem rodape"), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is False


# Panes de sessao Pi (`tmux capture-pane -p`, pi 0.82.1, capturados de uma sessao descartavel — o
# repositorio e publico, nada de conversa do usuario aqui). O boot nao tem UMA moldura; o TUI pronto
# desenha o chrome do composer. Sem provider="pi" o pane nunca casa as marcas do rodape do Claude e
# TODO envio esperava os 12s inteiros de timeout antes de digitar.
_PI_BOOT = " Warning: tmux extended-keys is off. Modified Enter keys may not work.\n"
# Variante CAIXA (pi antigo / outro tema): o que o marcador `╰─` original media.
_PI_IDLE = (
    " Reloaded keybindings, extensions, skills, prompts, themes, and context files\n"
    "\n"
    "╭" + "─" * 97 + "╮\n"
    "\n"
    "╰" + "─" * 97 + "╯\n"
)
# Variante REGUA — o desenho de HOJE, tanto no pi puro (`--no-extensions`) quanto com o pacote
# `pi-claude-code-ui`: duas reguas e a statusline, ZERO `╰─`. Foi este pane que fez cada mensagem
# custar 12s.
_PI_IDLE_RULES = (
    " ✻ Turn took 11s (Total time 3h 43m 36s · 12 turns)\n"
    "\n"
    "─" * 100 + "\n"
    "\n"
    "─" * 100 + "\n"
    "🤖 cline-pass/glm-5.2 (xhigh) │ 📁 piprobe │ 📟 cp-pi-probe │ 💬 sessão 0in/0out · ctx 0/1M\n"
)


def test_wait_input_ready_pi_pronto_retorna_na_primeira_leitura():
    with patch.object(ti, "_capture", lambda name: _PI_IDLE), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0, provider="pi") is True


def test_wait_input_ready_pi_composer_de_reguas_e_pronto():
    # Regressao do bug dos 12s: a UI atual do Pi desenha reguas, nao caixa. Se um dia mudar de novo,
    # e ESTE teste que quebra — em vez de o app so ficar lento e calado.
    with patch.object(ti, "_capture", lambda name: _PI_IDLE_RULES), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0, provider="pi") is True


def test_wait_input_ready_pi_timeout_avisa_uma_vez(caplog):
    # Marcador que para de casar nao pode ser silencioso: e assim que 12s por mensagem passam batido.
    ti._READY_TIMEOUT_WARNED.clear()
    with patch.object(ti, "_capture", lambda name: _PI_BOOT), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         caplog.at_level("WARNING", logger="claude_pocket.terminal_input"):
        assert ti._wait_input_ready("s", timeout=0.0, provider="pi") is False
        assert ti._wait_input_ready("s", timeout=0.0, provider="pi") is False
    assert len(caplog.records) == 1
    assert "pi" in caplog.records[0].getMessage()
    ti._READY_TIMEOUT_WARNED.clear()


def test_wait_input_ready_timeout_do_pi_e_menor_que_o_do_claude():
    # A espera so compra seguranca no boot (~4.3s medidos ate o composer); no estouro a gente envia
    # mesmo assim, entao teto menor = pior caso menor no dia em que o marcador desandar de novo.
    assert ti._TIMEOUTS_BY_PROVIDER["pi"] < ti._DEFAULT_TIMEOUT


def test_wait_input_ready_pi_bootando_nao_diz_pronto():
    # Sem a caixa do composer o TUI ainda nao aceita teclas -> a msg seria engolida.
    with patch.object(ti, "_capture", lambda name: _PI_BOOT), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0, provider="pi") is False


def test_wait_input_ready_claude_inalterado():
    # O default NAO pode mudar: rodape presente -> pronto; ausente -> espera e devolve False.
    with patch.object(ti, "_capture", lambda name: "algo\n? for shortcuts"), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is True
    with patch.object(ti, "_capture", lambda name: _PI_IDLE), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is False


def test_send_prompt_leva_o_provider_ate_o_gate():
    # Trava contra alguem DERRUBAR o argumento no meio do caminho: o gate roda de verdade (nao e
    # mockado fora), so espiamos com que provider ele foi chamado.
    real = ti._wait_input_ready
    seen = {}

    def spy(name, timeout=12.0, provider="claude"):
        seen["provider"] = provider
        return real(name, timeout=0.0, provider=provider)

    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: _PI_IDLE), \
         patch.object(ti, "_wait_input_ready", spy), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", lambda name, k, **kw: None):
        assert ti.TerminalInput().send_prompt("s", "oi", "pi") == "sent"
    assert seen["provider"] == "pi"


def test_drain_leva_o_provider_ate_o_send_prompt(tmp_path, monkeypatch):
    # O caminho da FILA (drain) e o segundo ponto de entrada: sem repassar, a msg enfileirada de uma
    # sessao Pi pagava os 12s mesmo com o /input corrigido.
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    pqueue.PromptQueue("s").append("oi", delivered=False)
    seen = []
    with patch.object(ti.TerminalInput, "send_prompt",
                      lambda self, name, text, provider="claude": seen.append(provider) or "sent"), \
         patch.object(ti, "_transcript_start_ts", lambda j: 0.0):
        assert ti.drain("s", "/nao/existe.jsonl", "pi") == 1
    assert seen == ["pi"]


def test_single_question_no_review_submits_without_escape():
    # Pergunta UNICA: o Enter da selecao ja submete; NAO ha tela de "Submit answers". O passo final
    # nao pode mandar Escape (interromperia o Claude que ja recebeu a resposta -> bug "aceitou mas
    # chegou errado"). Cursor abre na linha 1; Down x2 -> linha 3 (= indice 2 + 1) -> guard passa.
    keys = []
    submitted = {"v": False}

    def cap(name):
        if submitted["v"]:
            return "❯ \n⏵⏵ bypass permissions on"          # picker fechou: ja submeteu
        return "Pick one\n❯ 3. OPT-TWO\n  4. OPT-THREE\nEnter to select · Esc to cancel"

    def send(name, k, **kw):
        keys.append(k)
        if k == "Enter":
            submitted["v"] = True

    with patch.object(ti, "send_keys", send), patch.object(ti, "_capture", cap):
        ti.answer_questions("s", [{"kind": "option", "indices": [2], "multi": False, "labels": ["OPT-TWO"]}])
    assert keys == ["Down", "Down", "Enter"]   # navegou e submeteu
    assert "Escape" not in keys                 # SEM Escape espurio (era o interrupt do bug)


def test_single_question_nav_drift_self_corrects_then_submits():
    # Um Down engolido no redraw: cursor fica na linha 2 quando esperavamos a 3. Malha fechada: le a
    # linha real, manda o delta (1 Down) e re-le — corrigiu -> Enter submete. Drift vira ruido, nao erro.
    keys = []
    caps = iter([
        "Pick one\n❯ 2. OPT-ONE\n  3. OPT-TWO\nEnter to select · Esc to cancel",  # guard: drift (2 != 3)
        "Pick one\n  2. OPT-ONE\n❯ 3. OPT-TWO\nEnter to select · Esc to cancel",  # re-le: corrigido
        "❯ \n⏵⏵ bypass permissions on",                                            # picker fechou: submeteu
    ])
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: next(caps)):
        ti.answer_questions("s", [{"kind": "option", "indices": [2], "multi": False, "labels": ["OPT-TWO"]}])
    assert keys == ["Down", "Down", "Down", "Enter"]  # 2 cegos + 1 correcao + submit
    assert "Escape" not in keys


def test_single_question_nav_drift_unrecoverable_raises_drive_error():
    # Cursor preso na linha 2 apos 3 correcoes: DriveError SEM Enter (nao submete errado) e SEM
    # Escape (o Escape solto virava "user declined/interrupted" — agora e o caller que decide,
    # mandando Escape + resposta por texto).
    import pytest
    keys = []
    drift = "Pick one\n❯ 2. OPT-ONE\n  3. OPT-TWO\nEnter to select · Esc to cancel"
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: drift):
        with pytest.raises(ti.DriveError):
            ti.answer_questions("s", [{"kind": "option", "indices": [2], "multi": False, "labels": ["OPT-TWO"]}])
    assert "Enter" not in keys and "Escape" not in keys  # inerte: nada submetido, nada interrompido


def test_multi_question_review_submits():
    # Multiplas perguntas: ai SIM existe a tela "Submit answers". Guard por pergunta (linha do cursor) +
    # review final que bate os labels -> Enter submete. Sequencia de capturas: guard Q1, guard Q2, review.
    keys = []
    caps = iter([
        "First q\n❯ 2. A-ONE\n  3. A-TWO\nEsc to cancel",   # Q1: Down x1 -> linha 2 (indice 1)
        "Second q\n❯ 1. B-ZERO\n  2. B-ONE\nEsc to cancel",  # Q2: Down x0 -> linha 1 (indice 0)
        "Review your answers\n ● First q\n   → A-ONE\n ● Second q\n   → B-ZERO\n❯ 1. Submit answers\n  2. Cancel\n",
    ])
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: next(caps)):
        ti.answer_questions("s", [
            {"kind": "option", "indices": [1], "multi": False, "labels": ["A-ONE"]},
            {"kind": "option", "indices": [0], "multi": False, "labels": ["B-ZERO"]},
        ])
    assert keys == ["Down", "Enter", "Enter", "Enter"]  # Q1 Down+Enter, Q2 Enter, submit Enter
    assert "Escape" not in keys


def test_multi_select_macro_and_mismatch_raises_drive_error():
    keys = []
    bad_review = "Review your answers\n ● Q1\n   → Z\n❯ 1. Submit answers\n  2. Cancel\n"
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: bad_review):
        with pytest.raises(ti.DriveError):
            ti.answer_questions("s", [{"kind": "option", "indices": [0, 1], "multi": True, "labels": ["X", "Y"]}])
    # multi: Space, Down, Space, Right ; verify falha (review tem Z, nao X/Y) -> DriveError sem
    # Escape (caller faz Escape + fallback texto) e sem Enter (nunca submeteu)
    assert keys == ["Space", "Down", "Space", "Right"]


def test_text_without_value_raises_before_any_key():
    keys = []
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: ""):
        with pytest.raises(ValueError):
            ti.answer_questions("s", [{"kind": "text", "type_index": 3}])  # value ausente
    assert keys == []  # NENHUMA tecla enviada — TUI intocado


def test_empty_indices_raises_before_any_key():
    keys = []
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: ""):
        with pytest.raises(ValueError):
            ti.answer_questions("s", [{"kind": "option", "indices": [], "multi": False, "labels": ["X"]}])
    assert keys == []


def test_review_substring_not_false_positive():
    # label "A" NAO deve casar dentro de "Apply" no review
    keys = []
    review = "Review your answers\n ● Q1\n   → Apply\n❯ 1. Submit answers\n  2. Cancel\n"
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: review):
        with pytest.raises(ti.DriveError):  # "A" nao e token exato de "Apply" -> mismatch
            ti.answer_questions("s", [{"kind": "option", "indices": [0], "multi": False, "labels": ["A"]}])
    assert "Escape" not in keys  # inerte: fallback e do caller


def test_send_prompt_troca_surrogate_solto_antes_do_tmux():
    # Meio emoji no argv do `tmux send-keys` estoura UnicodeEncodeError (subprocess encoda em
    # utf-8) — e como é um ValueError, o caller traduzia pra 400 "control characters" e a msg do
    # usuário morria ali, antes até de entrar na fila. Vira U+FFFD e segue.
    ready = "❯ \n⏵⏵ bypass permissions on (shift+tab to cycle)"
    keys = []
    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: ready), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)):
        assert ti.TerminalInput().send_prompt("s", "corte \ud83d") == "sent"
    assert keys == ["corte �", "Enter"]
    keys[0].encode("utf-8")   # o que o subprocess faria com o argv


# Rodape REAL do claude v2.1.218 (medido no pane desta maquina): regua + 3 linhas de statusline + a
# linha de modo. E o pane que o gate tem que aprovar.
_CLAUDE_RODAPE = (
    "─" * 100 + "\n"
    "  🤖 Opus5 (high✦) │ 📁 claude-cockpit [main*] │ 📟 cc-2 │ ⎈ k8s-dev\n"
    "  💬 236k/600 240k/1M │ 💵 $22.83 │ ⚡5h:20% ↺57m │ 🕐 09:32 ⏱ 14h46m\n"
    "  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← 2 agents\n"
)


def test_ready_ignora_frase_mode_on_no_texto_da_conversa():
    # "mode on" era marcador, e e frase comum: prosa citando "auto mode on"/"debug mode on" fazia o
    # gate liberar com a TUI ainda bootando -> mensagem engolida, o proprio bug que ele existe pra
    # evitar. Agora o marcador do Claude e o GLIFO de modo, que nao aparece em prosa.
    pane = (
        "  o rodape novo diz auto mode on, e no outro modo manual mode on\n"
        "  (ligamos o debug mode on ali tambem)\n"
        + "\n" * 20      # boot: nenhum rodape desenhado ainda
    )
    with patch.object(ti, "_capture", lambda name: pane), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is False


def test_ready_casa_nos_dois_glifos_de_modo():
    # Os dois rodapes medidos no claude v2.1.218. Se o glifo mudar, e ESTE teste que quebra.
    for rodape in ("⏵⏵ auto mode on (shift+tab to cycle) · ← for agents",
                   "⏸ manual mode on · ← for agents"):
        with patch.object(ti, "_capture", lambda name, r=rodape: "conversa\n" * 30 + r), \
             patch.object(ti.time, "sleep", lambda *_: None):
            assert ti._wait_input_ready("s", timeout=0.0) is True, rodape


def test_ready_casa_no_rodape_de_verdade():
    # O par do teste acima: mesma conversa citando os marcadores, mas AGORA com o rodape real no fim.
    pane = "  citando `auto mode on` no meio da conversa\n" + "\n" * 30 + _CLAUDE_RODAPE
    with patch.object(ti, "_capture", lambda name: pane), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is True


def test_ready_rodape_com_linhas_em_branco_no_fim():
    # Redraw deixa linhas vazias depois do rodape; sem o rstrip elas empurrariam o rodape pra fora da
    # janela da cauda e cada envio voltaria a queimar os 12s de timeout, calado.
    with patch.object(ti, "_capture", lambda name: _CLAUDE_RODAPE + "\n" * 6), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is True


def test_send_prompt_partial_nao_manda_enter():
    # Envio literal que para no meio -> "partial", e o Enter NAO vai: submeter texto com buraco faria a
    # sessao agir sobre um pedido que o usuario nunca escreveu (foi o estrago original do truncamento).
    keys = []

    def falso_send_keys(name, k, **kw):
        keys.append(k)
        return not kw.get("literal")     # literal falha; tecla nomeada (Enter) daria True

    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: _CLAUDE_RODAPE), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", falso_send_keys):
        assert ti.TerminalInput().send_prompt("s", "oi") == "partial"
    assert keys == ["oi"]                # digitou (parcial) e PAROU: nenhum "Enter"


def test_drain_partial_nao_redigita_em_cima_do_residuo(tmp_path, monkeypatch, caplog):
    # O furo do `except Exception` cego: entrega parcial ficava sem log nenhum e o reconcile depois
    # requeava, fazendo o drain digitar o texto INTEIRO em cima do residuo cortado que ficou no
    # composer (nada limpa a linha). Agora para no primeiro parcial, com log de erro.
    from app.pqueue import PromptQueue
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    q = PromptQueue("s")
    q.append("mensagem longa", delivered=False)
    envios = []

    def send_prompt_parcial(self, name, text, provider="claude"):
        envios.append(text)
        return "partial"

    with patch.object(ti.TerminalInput, "send_prompt", send_prompt_parcial), \
         patch.object(ti, "_transcript_start_ts", lambda j: 0.0), \
         caplog.at_level("ERROR"):
        assert ti.drain("s", "/tmp/x.jsonl") == 0     # nao conta como entregue
    assert envios == ["mensagem longa"]               # UMA tentativa, sem repetir
    assert any("PARCIAL" in r.getMessage() for r in caplog.records)


# Composer VAZIO (submeteu) e composer COM RESIDUO (nao submeteu), no formato real do pane: o prompt
# fica entre duas reguas, com a statusline e a linha de modo abaixo.
def _pane_com_composer(conteudo: str) -> str:
    return ("  eco da conversa aqui\n"
            + "─" * 100 + "\n"
            + f"❯ {conteudo}\n"
            + "─" * 100 + "\n"
            "  🤖 Opus5 │ 📁 x\n"
            "  ⏵⏵ auto mode on (shift+tab to cycle)\n")


def test_composer_residuo_detecta_o_que_nao_submeteu():
    texto = "linha um\nlinha dois\nlinha final do recado"
    assert ti._composer_residuo(_pane_com_composer("linha final do recado"), texto) is True
    assert ti._composer_residuo(_pane_com_composer(""), texto) is False


def test_composer_residuo_ignora_digitacao_do_usuario():
    # Usuario digitou algo NOSSO nao e: comparar com a cauda do nosso texto evita o falso positivo.
    assert ti._composer_residuo(_pane_com_composer("outra coisa que eu digitei"),
                                "linha um\nlinha final do recado") is False


def test_composer_residuo_pane_ilegivel_nunca_inventa_falha():
    # Sem linha de prompt (pane em overlay/ilegivel) -> False: degrada pro comportamento de hoje.
    assert ti._composer_residuo("tela sem prompt nenhum\nnada aqui", "qualquer texto") is False
    assert ti._composer_residuo("", "qualquer texto") is False


def test_send_prompt_multilinha_que_nao_submete_devolve_partial():
    # O caso dos 3 recados longos que sairam com delivered=True e nunca viraram entrada no transcript
    # do destino (attempts=2 na fila, achados so lendo o sidecar).
    texto = "primeira linha\nsegunda linha\ncauda que fica no composer"
    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: _pane_com_composer("cauda que fica no composer")), \
         patch.object(ti.tmux, "paste_text", lambda name, t: None), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", lambda name, k, **kw: True):
        assert ti.TerminalInput().send_prompt("s", texto) == "partial"


def test_send_prompt_multilinha_que_submete_devolve_sent():
    # O par: composer limpo depois do Enter -> "sent", comportamento de sempre.
    texto = "primeira linha\nsegunda linha\ncauda"
    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: _pane_com_composer("")), \
         patch.object(ti.tmux, "paste_text", lambda name, t: None), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", lambda name, k, **kw: True):
        assert ti.TerminalInput().send_prompt("s", texto) == "sent"


def test_composer_residuo_ignora_o_eco_da_mensagem_submetida():
    # O erro que eu quase deixei passar: no Claude Code o ECO da msg ja submetida tambem comeca com ❯.
    # Pegando "a ultima linha com ❯" um redraw incompleto faria o eco valer como composer -> falso
    # "nao submeteu" num envio que DEU CERTO (usuario reenvia, msg duplica). O eco fica ACIMA das
    # reguas; a regiao do composer e entre as duas ultimas.
    texto = "linha um\ncauda do recado"
    pane = ("❯ cauda do recado\n"          # <- ECO, ja submetido
            "✻ Pensando…\n"
            + "─" * 100 + "\n"
            "❯\n"                            # <- composer VAZIO
            + "─" * 100 + "\n"
            "  ⏵⏵ auto mode on\n")
    assert ti._composer_residuo(pane, texto) is False


def test_composer_residuo_sem_reguas_nao_arrisca():
    # Pane sem as duas reguas (overlay, TUI de outro desenho) -> False: nunca inventa falha.
    assert ti._composer_residuo("❯ cauda do recado\nsem reguas aqui", "x\ncauda do recado") is False


def test_submeteu_tolera_leitura_stale_na_primeira_tentativa():
    # O furo que o reviewer achou: se a captura correr ANTES de qualquer redraw, a tela e a VELHA (texto
    # inteiro no composer) — indistinguivel de "nao submeteu". Foto unica reportaria parcial num envio
    # que DEU CERTO. Insistindo, a 2a leitura ja mostra o composer limpo e devolve sent.
    texto = "linha um\ncauda do recado"
    telas = [_pane_com_composer("cauda do recado"),   # stale: redraw ainda nao aconteceu
             _pane_com_composer("")]                  # ja submetido
    with patch.object(ti, "_capture", lambda name: telas.pop(0) if len(telas) > 1 else telas[0]), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._submeteu("s", texto) is True


def test_submeteu_desiste_no_prazo_quando_o_residuo_persiste():
    # O caso de verdade: a cauda continua no composer o tempo todo -> False (parcial).
    texto = "linha um\ncauda do recado"
    with patch.object(ti, "_capture", lambda name: _pane_com_composer("cauda do recado")), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._submeteu("s", texto) is False


def test_composer_ilegivel_avisa_uma_vez_por_sessao(caplog):
    # Checagem que morre calada e o mesmo estrago do marcador de TUI que para de casar: sem as reguas
    # ela fica inerte PRA SEMPRE e ninguem descobre. Avisa uma vez, nao por envio.
    ti._COMPOSER_WARNED.clear()
    pane = "tela sem regua nenhuma\nnada aqui"
    with caplog.at_level("WARNING", logger="claude_pocket.terminal_input"):
        # cauda longa o bastante pra passar do _RESIDUO_MIN e chegar na checagem das reguas
        assert ti._composer_residuo(pane, "x\ncauda longa do recado aqui", "sessao-x") is False
        assert ti._composer_residuo(pane, "x\ncauda longa do recado aqui", "sessao-x") is False
    assert len([r for r in caplog.records if "composer" in r.getMessage()]) == 1
    ti._COMPOSER_WARNED.clear()


def test_composer_residuo_nao_acusa_com_pane_real_do_repo():
    # Fixture REAL do repo (pane_idle.txt): tem reguas nas linhas 1, 4, 11, 45 e 47 de 51 — as de cima
    # sao divisoria do banner de boas-vindas. Confiar em "as duas ultimas reguas" quebra num redraw sem
    # a regua de BAIXO: o par viraria [11, 45] e a regiao seria a conversa inteira, com o ECO da propria
    # mensagem dentro -> falso "nao submeteu". As travas de fundo/altura recusam esse par.
    from pathlib import Path
    pane = Path(__file__).parent.joinpath("fixtures/pane_idle.txt").read_text(encoding="utf-8")
    linhas = pane.split("\n")
    # 1) tela inteira e coerente: nao acusa nada que nao foi digitado
    assert ti._composer_residuo(pane, "texto que nunca foi digitado nesta sessao") is False
    # 2) redraw SEM a regua de baixo: o par cairia no banner -> tem de virar ilegivel, nao acusar
    sem_ultima = "\n".join(l for i, l in enumerate(linhas) if i != 45 + 2)
    trecho = [l for l in linhas[12:44] if len(l.strip()) > 25]
    if trecho:                      # usa uma linha REAL do meio da conversa como "cauda"
        assert ti._composer_residuo(sem_ultima, "x\n" + trecho[0].strip()) is False


def test_composer_residuo_pega_cauda_quebrada_por_wrap():
    # Recado longo de um paragrafo so passa de 200 colunas e a TUI QUEBRA a linha na exibicao; um
    # `cauda in composer` cru falhava exatamente na classe de mensagem que motivou o conserto.
    texto = "recado longo numa linha so que termina com esta frase bem comprida aqui"
    pane = ("  eco\n" + "─" * 100 + "\n"
            "❯ recado longo numa linha so que termina com esta frase\n"
            "  bem comprida aqui\n"                      # <- wrap no meio da cauda
            + "─" * 100 + "\n"
            "  ⏵⏵ auto mode on\n")
    assert ti._composer_residuo(pane, texto) is True
