from unittest.mock import patch, call

import pytest
import time

from app import pqueue
from app import terminal_input
from app.pqueue import PromptQueue
from app.terminal_input import TerminalInput


@pytest.fixture
def tmp_queue(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_send_prompt_literal_then_enter():
    # Gate: pane entregavel (sessao viva, sem overlay) + marcador de ready -> envia e devolve "sent".
    # _entrou_no_composer stubado: o ponto DESTE teste e a ordem digita->Enter, nao a evidencia de
    # ingestao (essa tem teste proprio em test_terminal_answer.py). Sem o stub o pane falso nunca
    # mostraria o texto no composer e o envio viraria "partial".
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value="? for shortcuts\n"), \
         patch.object(terminal_input, "_entrou_no_composer", lambda *_a: True), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("cc", "corrige o bug") == "sent"
    assert sk.call_args_list == [
        call("cc", "corrige o bug", literal=True),
        call("cc", "Enter"),
    ]


def test_send_prompt_defers_on_overlay():
    # Overlay aberto (rodape de navegacao) -> NAO digita as cegas; devolve "deferred", zero teclas.
    pane = "● plano\n────────\n  Esc to cancel · Enter to select\n"
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value=pane), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("cc", "oi") == "deferred"
    sk.assert_not_called()


def test_send_prompt_rejects_control_chars():
    with pytest.raises(ValueError):
        TerminalInput().send_prompt("cc", "bad\x00null")


def test_deliverable_false_when_no_session(monkeypatch):
    monkeypatch.setattr(terminal_input.tmux, "has_session", lambda name: False)
    assert terminal_input.deliverable("cc") is False


def test_deliverable_true_on_capture_error(monkeypatch):
    monkeypatch.setattr(terminal_input.tmux, "has_session", lambda name: True)
    def boom(name, lines=200):
        raise OSError("capture falhou")
    monkeypatch.setattr(terminal_input.tmux, "capture_pane", boom)
    assert terminal_input.deliverable("cc") is True   # degrada pro envio de hoje, sem regressao


def test_drain_sends_pending_and_marks_delivered(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    PromptQueue("cc").append("dois", delivered=False)
    sent = []
    monkeypatch.setattr(
        terminal_input.TerminalInput, "send_prompt",
        lambda self, name, text, provider="claude", pane_id=None, msg_id=None: sent.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 2
    assert sent == ["um", "dois"]
    assert all(e["delivered"] for e in PromptQueue("cc").load())


def test_drain_noop_and_reverts_when_overlay(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    monkeypatch.setattr(
        terminal_input.TerminalInput, "send_prompt",
        lambda self, name, text, provider="claude", pane_id=None, msg_id=None: "deferred")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert PromptQueue("cc").load()[0]["delivered"] is False   # revertida (nao perdida)


def test_drain_does_not_revert_on_send_failure(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    def boom(self, name, text, provider="claude"):
        raise RuntimeError("tty caiu no meio")
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt", boom)
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    # at-most-once: permanece True -> NAO re-enfileira -> nao digita 2x um prompt nao-idempotente.
    assert PromptQueue("cc").load()[0]["delivered"] is True


def test_drain_cheap_check_skips_capture_when_nothing_pending(tmp_queue, monkeypatch):
    PromptQueue("cc").append("ja entregue", delivered=True)
    called = []
    monkeypatch.setattr(
        terminal_input.TerminalInput, "send_prompt",
        lambda self, name, text, provider="claude", pane_id=None, msg_id=None: called.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert called == []   # nem chamou send_prompt (e nem capture_pane)


def test_drain_skips_entries_before_start_ts(tmp_queue, tmp_path, monkeypatch):
    PromptQueue("cc").append("velha", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2999-01-01T00:00:00Z"}\n', encoding="utf-8")  # start_ts > ts da entrada
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text, provider="claude", pane_id=None: sent.append(text) or "sent")
    assert terminal_input.drain("cc", str(j)) == 0 and sent == []


def test_select_option_three_navigates_then_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().select("cc", 3)
    assert sk.call_args_list == [call("cc", "Down"), call("cc", "Down"), call("cc", "Enter")]


def test_select_option_one_just_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().select("cc", 1)
    assert sk.call_args_list == [call("cc", "Enter")]


def test_interrupt_sends_escape():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().interrupt("cc")
    assert sk.call_args_list == [call("cc", "Escape")]


# --- _composer_ocupado_pi: guarda anti-colagem (caso real ABC-1234: aviso de grupo ficou no
# composer com Enter engolido e o prompt do cockpit foi digitado em cima, virando UMA mensagem) ---

_REGUA = "─" * 60


def _pane_pi(composer_lines):
    corpo = ["pi v0.83.0", "conversa...", ""]
    return "\n".join(corpo + [_REGUA] + composer_lines + [_REGUA, "🤖 k3 status"])


def test_composer_pi_com_residuo_adia():
    with patch.object(terminal_input, "_capture", return_value=_pane_pi(["texto parado no composer"])):
        assert terminal_input._composer_ocupado_pi("pi-x") is True


def test_composer_pi_vazio_envia():
    with patch.object(terminal_input, "_capture", return_value=_pane_pi([])):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_composer_pi_ilegivel_nao_bloqueia():
    # pane sem réguas (redraw/boot): na dúvida envia — mesma política do resto do arquivo
    with patch.object(terminal_input, "_capture", return_value="pi v0.83.0\nsem reguas aqui"):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_send_prompt_pi_adia_com_residuo(monkeypatch):
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda name, provider="claude": True)
    with patch.object(terminal_input, "_capture", return_value=_pane_pi(["residuo de enter engolido"])), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("pi-x", "mensagem nova", provider="pi") == "deferred"
    sk.assert_not_called()   # nada digitado por cima do residuo


def test_drain_poda_entradas_de_vida_anterior_da_sessao(tmp_queue, tmp_path, monkeypatch):
    # Sessao morreu devendo (entrada pendente), tmux recriado com o MESMO nome (mesma pasta),
    # transcript RETOMADO (`pi -c` -> start_ts velho). A entrada da vida anterior NAO entrega.
    PromptQueue("cc").append("recado pra sessao que morreu", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2000-01-01T00:00:00Z"}\n', encoding="utf-8")  # transcript velho
    monkeypatch.setattr(terminal_input.tmux, "session_created", lambda name: time.time() + 60)
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text, provider="claude", pane_id=None: sent.append(text) or "sent")
    assert terminal_input.drain("cc", str(j)) == 0 and sent == []
    assert PromptQueue("cc").load() == []  # PODADA de verdade (nao marcada entregue, nao pendente)


def test_drain_entrega_entrada_mais_nova_que_o_tmux(tmp_queue, tmp_path, monkeypatch):
    # A direcao arriscada do max(): entrada enfileirada DEPOIS do nascimento do tmux atual e
    # mensagem viva — o corte novo nao pode come-la.
    PromptQueue("cc").append("mensagem viva", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2000-01-01T00:00:00Z"}\n', encoding="utf-8")
    monkeypatch.setattr(terminal_input.tmux, "session_created", lambda name: time.time() - 60)
    sent = []
    monkeypatch.setattr(
        terminal_input.TerminalInput, "send_prompt",
        lambda self, name, text, provider="claude", pane_id=None, msg_id=None: sent.append(text) or "sent")
    assert terminal_input.drain("cc", str(j)) == 1 and sent == ["mensagem viva"]


# --- identidade do placeholder de paste (achado CRITICO da review 31/07) ---

_REGUA_R = "─" * 60


def _pane_claude(composer_lines):
    return "\n".join(["banner", ""] + [_REGUA_R] + composer_lines + [_REGUA_R, "status"])


def test_paste_alheio_nao_conta_como_entrega():
    # Placeholder que JA existia (rascunho do usuario) nao pode virar prova da nossa mensagem —
    # o Enter submeteria texto de terceiro.
    pane = _pane_claude(["❯ [Pasted text #1 +12 lines]"])
    r = terminal_input._composer_residuo(pane, "mensagem longa que nunca foi colada de verdade",
                                         "cc", pastes_antes={"1"})
    assert r is not True


def test_paste_novo_conta_como_entrega():
    pane = _pane_claude(["❯ [Pasted text #2 +3 lines]"])
    r = terminal_input._composer_residuo(pane, "mensagem longa entregue via paste colapsado",
                                         "cc", pastes_antes={"1"})
    assert r is True


def test_sem_foto_previa_placeholder_nao_conta():
    # pastes_antes=None (ramo de linha unica / caminhos antigos): placeholder nunca e prova.
    pane = _pane_claude(["❯ [Pasted text #7 +2 lines]"])
    r = terminal_input._composer_residuo(pane, "texto longo o bastante pra ter cauda valida", "cc")
    assert r is not True


# --- ramo de UMA LINHA + paste colapsado (bug medido 01/08: acima de ~800 chars numa linha so, o
# Claude Code colapsa o texto em "[Pasted text #N ...]" e o texto real nunca e desenhado na tela).
# O ramo multi-linha ja tira foto dos placeholders ANTES do envio (pastes_antes) e repassa pras duas
# checagens; o ramo de uma linha nao tirava, entao a evidencia por placeholder ficava DESLIGADA
# (_composer_residuo so aceita a prova quando pastes_antes != None) e a busca pela cauda visivel
# falhava sempre -> "envio incompleto", nenhum Enter mandado.


def test_send_prompt_uma_linha_paste_colapsado_envia(monkeypatch):
    # Reproduz o bug ao vivo: texto de 1000 chars numa linha so, o pane devolve um placeholder NOVO
    # (sem a cauda do texto visivel) depois do envio. Tem que resultar em Enter mandado e "sent" —
    # hoje (antes do fix) devolve "partial" e nenhum Enter, porque o ramo de uma linha nao passa a
    # foto pre-envio pras checagens.
    clock = [1000.0]

    def fake_sleep(s):
        clock[0] += s

    monkeypatch.setattr(terminal_input.time, "sleep", fake_sleep)
    monkeypatch.setattr(terminal_input.time, "monotonic", lambda: clock[0])

    texto = "x" * 1000  # 1 linha, > ~800 chars -> Claude Code colapsa em placeholder (medido)
    estado = {"enviado": False, "enter": False}

    def capture(name):
        if not estado["enviado"]:
            composer = []                                    # composer vazio, antes do envio
        elif not estado["enter"]:
            composer = ["❯ [Pasted text #1 +0 lines]"]        # colapsou; texto real NAO aparece
        else:
            composer = []                                    # Enter limpou o composer
        return "\n".join(["banner", "", _REGUA_R] + composer + [_REGUA_R, "? for shortcuts"])

    def fake_send_keys(name, keys, literal=False):
        if literal and keys == texto:
            estado["enviado"] = True
        if keys == "Enter":
            estado["enter"] = True
        return True

    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch.object(terminal_input, "_capture", side_effect=capture), \
         patch.object(terminal_input, "send_keys", side_effect=fake_send_keys) as sk:
        resultado = TerminalInput().send_prompt("cc", texto)

    assert resultado == "sent"
    assert call("cc", "Enter") in sk.call_args_list


def test_send_prompt_uma_linha_placeholder_previo_nao_conta_como_entrega(monkeypatch):
    # Caso irmao (protecao contra regressao na direcao oposta): placeholder que JA estava no
    # composer ANTES do envio (rascunho do usuario) nao pode contar como a NOSSA entrega — e a
    # razao de existir da foto pre-envio (ver comentario de terminal_input.py:198-203: aceitar
    # placeholder alheio ja chegou a submeter texto de terceiro). Aqui o placeholder previo nunca
    # some e a cauda nunca aparece -> tem que ficar "partial", sem Enter.
    clock = [1000.0]

    def fake_sleep(s):
        clock[0] += s

    monkeypatch.setattr(terminal_input.time, "sleep", fake_sleep)
    monkeypatch.setattr(terminal_input.time, "monotonic", lambda: clock[0])

    texto = "mensagem curta que nunca aparece no composer simulado"

    def capture(name):
        # placeholder #1 sempre presente (rascunho previo do usuario) — nao muda com o envio.
        composer = ["❯ [Pasted text #1 +5 lines]"]
        return "\n".join(["banner", "", _REGUA_R] + composer + [_REGUA_R, "? for shortcuts"])

    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch.object(terminal_input, "_capture", side_effect=capture), \
         patch.object(terminal_input, "send_keys", return_value=True) as sk:
        resultado = TerminalInput().send_prompt("cc", texto)

    assert resultado == "partial"
    assert call("cc", "Enter") not in sk.call_args_list


# --- _diag_composer: diagnostico SO-LOG anexado aos casos de "partial"/"deferred" (nao muda
# nenhum retorno, so acrescenta contexto ao log de erro) ---


def test_diag_composer_pane_legivel_traz_regiao_cauda_e_geometria():
    pane = _pane_claude(["❯ resto da mensagem que ficou pela metade"])
    diag = terminal_input._diag_composer(pane, "mensagem que ficou pela metade", "cc", {"1"})
    assert "resto da mensagem que ficou pela metade" in diag  # regiao do composer
    assert "cauda='mensagem que ficou pela metade'" in diag or "cauda=" in diag
    assert "reguas=" in diag and "fundo=" in diag and "altura=" in diag
    assert "pastes(antes=['1']" in diag


def test_diag_composer_inclui_sessao_e_inicio():
    # Achados da review 02/08/2026: item MEDIO 3 (o diagnostico so mostrava a cauda, nunca o comeco
    # que o conserto passou a usar como prova — ver _RESIDUO_INICIO) e item BAIXO 6 (o parametro
    # `name` chegava e nunca era lido). Os dois resolvidos juntos: o diag agora traz `sessao=` e
    # `inicio=`.
    pane = _pane_claude(["❯ resto da mensagem que ficou pela metade"])
    diag = terminal_input._diag_composer(pane, "mensagem que ficou pela metade", "sessao-x", None)
    assert "sessao='sessao-x'" in diag
    assert "inicio=" in diag


def test_diag_composer_pane_ilegivel_nao_lanca():
    # Sem reguas -> _composer_regiao devolve None; o helper tem que descrever a ausencia, nao explodir.
    diag = terminal_input._diag_composer("tela sem nenhuma regua aqui", "oi", "cc", None)
    assert "ilegivel" in diag


def test_diag_composer_degrada_em_string_quando_algo_exploda(monkeypatch):
    # Forca uma excecao dentro do helper (ex.: _composer_regiao quebrado) -- tem que devolver uma
    # string curta em vez de propagar, porque isto roda no meio de um envio ja falho.
    def boom(pane, nome_sessao=""):
        raise RuntimeError("kaboom")
    monkeypatch.setattr(terminal_input, "_composer_regiao", boom)
    diag = terminal_input._diag_composer(_pane_claude(["❯ oi"]), "oi", "cc", None)
    assert diag.startswith("diag indisponivel:")
    assert "kaboom" in diag


def test_send_prompt_partial_loga_diagnostico_no_erro(monkeypatch, caplog):
    # Mesmo cenario de test_send_prompt_uma_linha_placeholder_previo_nao_conta_como_entrega (placeholder
    # alheio nunca conta, resultado "partial"), so que aqui a prova e o CONTEUDO do log: a linha de
    # erro tem que trazer o diagnostico (regiao/cauda/geometria), sem mudar o retorno "partial".
    clock = [1000.0]

    def fake_sleep(s):
        clock[0] += s

    monkeypatch.setattr(terminal_input.time, "sleep", fake_sleep)
    monkeypatch.setattr(terminal_input.time, "monotonic", lambda: clock[0])

    texto = "mensagem curta que nunca aparece no composer simulado"

    def capture(name):
        composer = ["❯ [Pasted text #1 +5 lines]"]
        return "\n".join(["banner", "", _REGUA_R] + composer + [_REGUA_R, "? for shortcuts"])

    with caplog.at_level("ERROR", logger="claude_pocket.terminal_input"), \
         patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch.object(terminal_input, "_capture", side_effect=capture), \
         patch.object(terminal_input, "send_keys", return_value=True):
        resultado = TerminalInput().send_prompt("cc", texto)

    assert resultado == "partial"   # comportamento identico ao de antes da instrumentacao
    [rec] = caplog.records
    msg = rec.getMessage()
    assert "envio PARCIAL" in msg
    assert "diag:" in msg
    assert "[Pasted text #1" in msg  # regiao do composer chegou no log


def test_send_prompt_pi_composer_vazio_envia(monkeypatch):
    # Regressao-trava: o gate novo do Pi NAO pode bloquear envio com composer vazio — o texto
    # tem que chegar no send_keys e o resultado ser "sent".
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda name, provider="claude": True)
    monkeypatch.setattr(terminal_input, "_entrou_no_composer",
                        lambda name, texto, pastes_antes=None: True)
    monkeypatch.setattr(terminal_input, "_submeteu",
                        lambda name, texto, pastes_antes=None: True)
    monkeypatch.setattr(terminal_input.time, "sleep", lambda s: None)
    pane_vazio = "\n".join(["conversa", _REGUA_R, _REGUA_R, "status"])
    teclas = []
    with patch.object(terminal_input, "_capture", return_value=pane_vazio), \
         patch.object(terminal_input, "send_keys",
                      side_effect=lambda name, keys, literal=False: teclas.append(keys) or True):
        assert TerminalInput().send_prompt("pi-x", "oi tudo bem", provider="pi") == "sent"
    assert "oi tudo bem" in teclas and "Enter" in teclas


# --- Conserto 02/08/2026, causa raiz 1 (Claude): o composer CORTA a EXIBICAO de texto longo — so
# desenha as primeiras linhas, a cauda nunca aparece. Log real (duas ocorrencias seguidas):
#   cauda='ude-pocket-uploads/1785666473-67f17f.png'
#   composer='───\n❯ [Image #1]Então aconteceu várias outras vezes aqui, mas imagino que seu oque é
#   , todas no pi ,\n  As vezes dps de rodar um subagnt ele aparece essa sugestão, não tem nd
#   digitado aí , seu eu for\n  pelo terminal e escrever vai , mas se tá usando a visualização no
#   pane talvez não quer enviar,\n  vi'
# O texto ESTAVA la (da pra ver o comeco) — so a cauda que nunca foi desenhada.


def test_send_prompt_texto_longo_com_comeco_visivel_e_cauda_cortada_envia(monkeypatch):
    # Reproduz a forma do log real: [Image #1] no comeco, texto que o composer so desenha ATE certo
    # ponto, e a cauda (fim do caminho da imagem colada) nunca aparece na tela. Antes do conserto isto
    # devolvia "partial" sem mandar Enter; agora o COMECO prova a entrega.
    clock = [1000.0]

    def fake_sleep(s):
        clock[0] += s

    monkeypatch.setattr(terminal_input.time, "sleep", fake_sleep)
    monkeypatch.setattr(terminal_input.time, "monotonic", lambda: clock[0])

    comeco = ("[Image #1]Então aconteceu várias outras vezes aqui, mas imagino que seu oque é , "
              "todas no pi ,")
    texto = (comeco + "\nAs vezes dps de rodar um subagnt ele aparece essa sugestão, não tem nd "
             "digitado aí\npelo terminal e escrever vai, mas se tá usando a visualização no pane\n"
             ".claude-pocket-uploads/1785666473-67f17f.png")

    estado = {"colado": False, "enter": False}

    def capture(name):
        if not estado["colado"] or estado["enter"]:
            composer = []                       # antes de colar, ou depois do Enter: vazio
        else:
            composer = ["❯ " + comeco]           # colado: SO o comeco e desenhado (cauda cortada)
        return "\n".join(["banner", "", _REGUA_R] + composer + [_REGUA_R, "? for shortcuts"])

    def fake_paste(name, t):
        estado["colado"] = True

    def fake_send_keys(name, keys, literal=False):
        if keys == "Enter":
            estado["enter"] = True
        return True

    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch.object(terminal_input, "_capture", side_effect=capture), \
         patch.object(terminal_input.tmux, "paste_text", side_effect=fake_paste), \
         patch.object(terminal_input, "send_keys", side_effect=fake_send_keys) as sk:
        resultado = TerminalInput().send_prompt("cc", texto)

    assert resultado == "sent"
    assert call("cc", "Enter") in sk.call_args_list


# --- Achado CRITICO da review 02/08/2026: paste_text tinha o retorno de _send_literal DESCARTADO.
# Com a prova por comeco (_RESIDUO_INICIO) sozinha, um paste que PARA no meio (ex.: a 2a de 3 linhas
# falha no fallback do Windows/psmux) ainda mostra o comeco no composer — a leitura da tela, sozinha,
# diria "entrou", o Enter iria, e send_prompt devolveria "sent" CALADO pra um texto pela metade.


def test_send_prompt_multilinha_com_falha_confirmada_no_meio_vira_partial(monkeypatch, caplog):
    clock = [1000.0]

    def fake_sleep(s):
        clock[0] += s

    monkeypatch.setattr(terminal_input.time, "sleep", fake_sleep)
    monkeypatch.setattr(terminal_input.time, "monotonic", lambda: clock[0])

    texto = "primeira linha\nsegunda linha que falha no meio\nterceira linha nunca digitada"

    # Composer mostra so o COMECO — a leitura de tela sozinha diria "entrou" (e e exatamente o
    # risco que a prova por comeco introduziu, se paste_text nao propagasse a falha).
    def capture(name):
        composer = ["❯ primeira linha"]
        return "\n".join(["banner", "", _REGUA_R] + composer + [_REGUA_R, "? for shortcuts"])

    with caplog.at_level("ERROR", logger="claude_pocket.terminal_input"), \
         patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch.object(terminal_input, "_capture", side_effect=capture), \
         patch.object(terminal_input.tmux, "paste_text", return_value=False), \
         patch.object(terminal_input, "send_keys", return_value=True) as sk:
        resultado = TerminalInput().send_prompt("cc", texto)

    assert resultado == "partial"
    assert call("cc", "Enter") not in sk.call_args_list
    assert any("PARCIAL" in r.getMessage() and "paste_text" in r.getMessage() for r in caplog.records)


def test_send_prompt_mensagem_curta_continua_enviando():
    # Regressao-trava: mensagem curta ("ok") tem cauda E comeco curtos demais pra provar qualquer
    # coisa (_RESIDUO_MIN) -> _composer_residuo devolve None -> segue enviando (politica de sempre:
    # na duvida, envia). Acrescentar a checagem por comeco NAO pode fazer isto parar de funcionar.
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value="? for shortcuts\n"), \
         patch.object(terminal_input, "send_keys", return_value=True) as sk:
        assert TerminalInput().send_prompt("cc", "ok") == "sent"
    assert call("cc", "Enter") in sk.call_args_list


def test_send_prompt_texto_que_nunca_chega_continua_partial(monkeypatch):
    # A protecao em si: se o texto REALMENTE nao chegou (composer sempre vazio, nem comeco nem cauda
    # aparecem), o Enter continua NAO sendo enviado. Se este teste cair, o conserto afrouxou a trava.
    clock = [1000.0]

    def fake_sleep(s):
        clock[0] += s

    monkeypatch.setattr(terminal_input.time, "sleep", fake_sleep)
    monkeypatch.setattr(terminal_input.time, "monotonic", lambda: clock[0])

    texto = "mensagem longa o bastante pra ter cauda e comeco validos, mas que nunca chega no pane"

    def capture(name):
        return "\n".join(["banner", "", _REGUA_R, _REGUA_R, "? for shortcuts"])  # composer sempre vazio

    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch.object(terminal_input, "_capture", side_effect=capture), \
         patch.object(terminal_input, "send_keys", return_value=True) as sk:
        resultado = TerminalInput().send_prompt("cc", texto)

    assert resultado == "partial"
    assert call("cc", "Enter") not in sk.call_args_list


def test_comeco_de_rascunho_alheio_nao_vira_prova():
    # Invariante critico (achado de 31/07, agora valendo tambem pro comeco): o usuario digitando o
    # PROPRIO rascunho no composer nao pode virar prova de que o NOSSO texto foi entregue.
    pane = _pane_claude(["❯ isso aqui e um rascunho que o usuario esta digitando agora mesmo"])
    r = terminal_input._composer_residuo(
        pane, "mensagem completamente diferente que o agente esta tentando entregar agora", "cc")
    assert r is not True


# --- Conserto 02/08/2026, causa raiz 2 (Pi): o aviso de subagente contava como "rascunho no
# composer" e adiava o envio PRA SEMPRE (o usuario confirmou: so destravava mexendo no terminal a
# mao). Log real: " Subagent async grouped result intercom delivery was not acknowledged for
# '/tmp/pi-subagents-uid-1000/async-subagent-results/a56523ed-40de-4fc7-a352-8fa39f29f908.json'."

_AVISO_SUBAGENT_PI = (
    "Subagent async grouped result intercom delivery was not acknowledged for "
    "'/tmp/pi-subagents-uid-1000/async-subagent-results/a56523ed-40de-4fc7-a352-8fa39f29f908.json'.")


def test_composer_pi_aviso_de_subagente_nao_conta_como_ocupado():
    with patch.object(terminal_input, "_capture", return_value=_pane_pi([_AVISO_SUBAGENT_PI])):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_send_prompt_pi_envia_apesar_do_aviso_de_subagente(monkeypatch):
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda name, provider="claude": True)
    monkeypatch.setattr(terminal_input, "_entrou_no_composer",
                        lambda name, texto, pastes_antes=None: True)
    monkeypatch.setattr(terminal_input, "_submeteu",
                        lambda name, texto, pastes_antes=None: True)
    monkeypatch.setattr(terminal_input.time, "sleep", lambda s: None)
    with patch.object(terminal_input, "_capture", return_value=_pane_pi([_AVISO_SUBAGENT_PI])), \
         patch.object(terminal_input, "send_keys", return_value=True) as sk:
        assert TerminalInput().send_prompt("pi-x", "Deu algum problema?", provider="pi") == "sent"
    assert call("pi-x", "Deu algum problema?", literal=True) in sk.call_args_list


def test_composer_pi_aviso_quebrado_em_duas_linhas_nao_conta_como_ocupado():
    # Achado da review 02/08/2026: a 1a versao exigia a frase inteira numa LINHA FISICA (`[^\n]*`).
    # O `capture-pane` devolve linha de TELA, e o aviso (~167 chars) quebra num pane estreito —
    # medido em 90 e 99 colunas. Quebra no boundary de PALAVRA (sem perder nem repetir caractere).
    linha1 = "Subagent async grouped result intercom delivery was not acknowledged for"
    linha2 = "'/tmp/pi-subagents-uid-1000/async-subagent-results/a56523ed-40de-4fc7-a352-8fa39f29f908.json'."
    with patch.object(terminal_input, "_capture", return_value=_pane_pi([linha1, linha2])):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_composer_pi_aviso_quebrado_em_tres_linhas_nao_conta_como_ocupado():
    # Variante mais dura: quebra no MEIO da palavra "acknowledged" e no meio de "results" — o caso
    # que um wrap cru (sem hifenizacao) realmente produz. Sem espaco/quebra nenhum foi
    # perdido ou duplicado, entao remover TODO whitespace reconstroi a sequencia original.
    linha1 = "Subagent async grouped result intercom delivery was not ackno"
    linha2 = "wledged for '/tmp/pi-subagents-uid-1000/async-subagent-resul"
    linha3 = "ts/a56523ed-40de-4fc7-a352-8fa39f29f908.json'."
    with patch.object(terminal_input, "_capture", return_value=_pane_pi([linha1, linha2, linha3])):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_composer_pi_rascunho_de_verdade_continua_ocupado():
    # O guarda original nao pode morrer: um rascunho de VERDADE (nao o aviso de sistema conhecido)
    # continua adiando o envio.
    with patch.object(terminal_input, "_capture",
                      return_value=_pane_pi(["preciso lembrar de perguntar sobre o deploy amanha"])):
        assert terminal_input._composer_ocupado_pi("pi-x") is True


def test_send_prompt_pi_ocupado_alem_do_limite_vira_erro_visivel(monkeypatch, caplog):
    # "Adiar pra sempre e pior que avisar": o aviso original (uma vez, WARNING) calava depois da
    # primeira tentativa enquanto a fila ficava emperrada — o usuario so descobria mexendo no
    # terminal a mao. Acima de _OCUPADO_DEFER_LIMIT tentativas seguidas, vira ERRO visivel (com
    # TREGUA — ver _avisa_deferred; achado MEDIO da review 02/08/2026: sem tregua isto inundava o
    # journal a cada tentativa, quebrando o padrao "avisa uma vez e cala" do resto do arquivo).
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda name, provider="claude": True)
    monkeypatch.setattr(terminal_input.time, "sleep", lambda s: None)
    pane_ocupado = _pane_pi(["rascunho que nunca sai do composer"])
    total = terminal_input._OCUPADO_DEFER_LIMIT * 2 + 1   # passa por DUAS viradas da tregua
    with caplog.at_level("ERROR", logger="claude_pocket.terminal_input"), \
         patch.object(terminal_input, "_capture", return_value=pane_ocupado), \
         patch.object(terminal_input, "send_keys") as sk:
        for _ in range(total):
            assert TerminalInput().send_prompt("pi-y", "mensagem", provider="pi") == "deferred"
    sk.assert_not_called()
    erros = [r for r in caplog.records if "composer do pi" in r.getMessage()]
    assert erros                                     # nao ficou mudo
    assert len(erros) < total - terminal_input._OCUPADO_DEFER_LIMIT   # e nao virou 1 erro por tentativa


def test_pi_com_linha_entrega_sem_digitar(monkeypatch):
    """O ponto da fase inteira: havendo linha, NENHUMA tecla é mandada."""
    from app import pi_inbox, terminal_input

    teclas = []
    monkeypatch.setattr(terminal_input, "send_keys", lambda *a, **k: teclas.append(a) or True)
    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", lambda pane: True)
    monkeypatch.setattr(pi_inbox.INBOX, "entregar_sync", lambda pane, texto, msg_id=None: "sent")

    r = terminal_input.TerminalInput().send_prompt("s", "oi", provider="pi", pane_id="%1")
    assert r == "sent"
    assert teclas == [], "com linha viva, nada pode ser digitado no tmux"


def test_pi_com_linha_funciona_mesmo_com_overlay(monkeypatch):
    """A linha não digita, então overlay aberto não é motivo pra adiar. Na v1 do plano o gate
    `deliverable` vinha antes e a sessão com picker nunca tentava a linha."""
    from app import pi_inbox, terminal_input

    monkeypatch.setattr(terminal_input, "deliverable", lambda name: False)
    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", lambda pane: True)
    monkeypatch.setattr(pi_inbox.INBOX, "entregar_sync", lambda pane, texto, msg_id=None: "sent")

    assert terminal_input.TerminalInput().send_prompt(
        "s", "oi", provider="pi", pane_id="%1") == "sent"


def test_pi_sem_linha_cai_na_tecla(monkeypatch):
    """Sessão Pi antiga (sem a extensão nova) continua funcionando exatamente como hoje."""
    from app import pi_inbox, terminal_input

    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", lambda pane: False)
    chamou = {"v": False}

    def marcar(*a, **k):
        chamou["v"] = True
        return False   # composer vazio: segue o fluxo normal de tecla

    monkeypatch.setattr(terminal_input, "_composer_ocupado_pi", marcar)
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda *a, **k: True)
    monkeypatch.setattr(terminal_input, "_entrou_no_composer", lambda *a, **k: True)
    monkeypatch.setattr(terminal_input, "_submeteu", lambda *a, **k: True)
    monkeypatch.setattr(terminal_input, "send_keys", lambda *a, **k: True)

    r = terminal_input.TerminalInput().send_prompt("s", "oi", provider="pi", pane_id="%1")
    assert r == "sent"
    assert chamou["v"] is True, "sem linha, o caminho de tecla (com o guarda) tem que rodar"


def test_pi_linha_sem_confirmacao_nao_digita(monkeypatch):
    """A regra da duplicata: tentou pela linha e não confirmou -> deferred, e NENHUMA tecla."""
    from app import pi_inbox, terminal_input

    teclas = []
    monkeypatch.setattr(terminal_input, "send_keys", lambda *a, **k: teclas.append(a) or True)
    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", lambda pane: True)
    monkeypatch.setattr(pi_inbox.INBOX, "entregar_sync", lambda pane, texto, msg_id=None: "deferred")

    r = terminal_input.TerminalInput().send_prompt("s", "oi", provider="pi", pane_id="%1")
    assert r == "deferred"
    assert teclas == []


def test_claude_nunca_consulta_a_linha(monkeypatch):
    """Sessão de Claude não pode nem encostar no caminho novo."""
    from app import pi_inbox, terminal_input

    def explode(*a, **k):
        raise AssertionError("o caminho do Pi foi consultado numa sessao Claude")

    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", explode)
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda *a, **k: True)
    monkeypatch.setattr(terminal_input, "_entrou_no_composer", lambda *a, **k: True)
    monkeypatch.setattr(terminal_input, "_submeteu", lambda *a, **k: True)
    monkeypatch.setattr(terminal_input, "send_keys", lambda *a, **k: True)

    assert terminal_input.TerminalInput().send_prompt("s", "oi") == "sent"


# --- id estavel entre reentregas pela linha do Pi (achado ALTA da revisao 02/08/2026) ------------
# A extensao (cp-state.ts) chama sendUserMessage ANTES de confirmar: se o ACK atrasa/perde,
# pi_inbox.entregar devolve "deferred" mas a instrucao JA pode ter chegado no agente. Sem um id
# ESTAVEL entre a 1a tentativa (_send_one, via api.py) e o retry (drain, abaixo), a extensao nao tem
# como reconhecer o retry como a MESMA mensagem e chamaria sendUserMessage de novo. Estes dois testes
# prova a plumbing do lado do backend (msg_id sobrevive ao round-trip fila -> drain -> send_prompt);
# a dedupe em si (guardar os ids ja entregues) mora em cp-state.ts, sem harness de teste TS no repo
# — verificada por leitura + execucao manual da logica extraida (ver relatorio).

def test_porta_a_retry_pela_fila_usa_o_mesmo_msg_id(tmp_queue, monkeypatch):
    """Timeout de ACK (linha viva, "deferred") -> drain() reclama a MESMA entrada -> a 'extensao'
    (dublê) recebe o MESMO id nas duas tentativas."""
    from app import pi_inbox, terminal_input
    from app.pqueue import PromptQueue

    ids_recebidos = []

    def fake_entregar_sync(pane, texto, msg_id=None):
        ids_recebidos.append(msg_id)
        return "deferred"   # ACK nunca chega, nas DUAS tentativas

    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", lambda pane: True)
    monkeypatch.setattr(pi_inbox.INBOX, "entregar_sync", fake_entregar_sync)
    # drain resolve pelo pane do AGENTE (I1 da revisao final), nao pelo ATIVO.
    monkeypatch.setattr(terminal_input.agentpane, "pane_info", lambda name: ("pi", "%1"))

    entry = PromptQueue("cc").append("oi", delivered=False)

    # 1a tentativa: o que _send_one faz hoje (api.py cria a entrada e passa o id como msg_id).
    r1 = terminal_input.TerminalInput().send_prompt(
        "cc", "oi", provider="pi", pane_id="%1", msg_id=entry["id"])
    assert r1 == "deferred"

    # 2a tentativa: drain() reclama a MESMA entrada (delivered ainda False) e tenta de novo.
    assert terminal_input.drain("cc", "/no/such.jsonl", "pi") == 0
    assert PromptQueue("cc").load()[0]["delivered"] is False   # segue pendente pro proximo drain

    assert len(ids_recebidos) == 2
    assert ids_recebidos[0] == ids_recebidos[1] == entry["id"], (
        "as DUAS tentativas tem que carregar o MESMO id -- e o que deixa a extensao reconhecer "
        "retry e nao repetir sendUserMessage (a dedupe em si mora em cp-state.ts)"
    )


def test_porta_b_reconcile_redrena_com_o_mesmo_msg_id(tmp_queue, monkeypatch):
    """_confirm_and_drain dispara pra QUALQUER 'sent', inclusive o da linha do Pi: se o texto nao
    aterrissar no transcript como committed_user_lines espera, reconcile_delivered re-enfileira a
    MESMA entrada e o drain() a reenvia -- tambem com o id ORIGINAL, senao seria a mesma duplicata
    da Porta A por outra porta."""
    from app import pi_inbox, terminal_input
    from app.pqueue import PromptQueue

    ids_recebidos = []

    def fake_entregar_sync(pane, texto, msg_id=None):
        ids_recebidos.append(msg_id)
        return "sent"

    monkeypatch.setattr(pi_inbox.INBOX, "tem_linha", lambda pane: True)
    monkeypatch.setattr(pi_inbox.INBOX, "entregar_sync", fake_entregar_sync)
    # drain resolve pelo pane do AGENTE (I1 da revisao final), nao pelo ATIVO.
    monkeypatch.setattr(terminal_input.agentpane, "pane_info", lambda name: ("pi", "%1"))

    # Como se _send_one ja tivesse marcado 'sent' (delivered=True) na 1a tentativa.
    entry = PromptQueue("cc").append("oi", delivered=True, ts=1000.0)

    q = PromptQueue("cc")
    requeued = q.reconcile_delivered(committed=set(), min_ts=0.0, now=1000.0 + 20, grace=8.0)
    assert requeued and requeued[0]["id"] == entry["id"], "reconcile preserva o id da entrada"

    assert terminal_input.drain("cc", "/no/such.jsonl", "pi") == 1   # 2a tentativa, agora "sent"
    assert ids_recebidos == [entry["id"]], "o reenvio do reconcile usa o MESMO id da 1a tentativa"
