import threading
from pathlib import Path
from unittest.mock import call, patch

import pytest

from app import model_picker as mp
from app import terminal_input
from app.terminal_input import TerminalInput

FIX = Path(__file__).parent / "fixtures"
# Panes reais capturados do picker ao vivo (tmux): Opus ativo (6 niveis de esforco, cursor
# na linha 2, esforco xHigh) e cursor sobre Haiku (esforco nao suportado).
PANE_OPUS = (FIX / "pane_model_picker_opus.txt").read_text()
PANE_HAIKU = (FIX / "pane_model_picker_haiku.txt").read_text()
# Pane de chat parado: e o que `_require_drivable` le ANTES de digitar `/model` (a sessao
# precisa estar livre — com o Claude trabalhando o texto viraria mensagem enfileirada).
PANE_IDLE = "assistant: pronto\n❯ \n"


# ── parse de linhas de modelo ────────────────────────────────────────────────
def test_parse_model_rows_opus_fixture():
    rows = mp.parse_model_rows(PANE_OPUS)
    assert [r["number"] for r in rows] == [1, 2, 3, 4]
    assert [r["keyword"] for r in rows] == ["default", "opus", "sonnet", "haiku"]
    # cursor (❯) e ativo (✔) ambos na linha 2 (Opus) ao abrir
    assert [r["cursor"] for r in rows] == [False, True, False, False]
    assert [r["active"] for r in rows] == [False, True, False, False]


def test_parse_model_rows_haiku_cursor_distinct_from_active():
    rows = mp.parse_model_rows(PANE_HAIKU)
    cur = mp.cursor_row(rows)
    assert cur["keyword"] == "haiku" and cur["cursor"] is True
    # ativo (✔) continua no Opus (linha 2), separado do cursor
    assert next(r for r in rows if r["active"])["keyword"] == "opus"


def test_picker_open_detection():
    assert mp.picker_open(PANE_OPUS) is True
    assert mp.picker_open("apenas chat sem picker\n❯ \n") is False


def test_parse_model_rows_ignores_chat_scrollback_numbered_list():
    # Lista numerada no historico do chat NAO deve virar linha de modelo (sem regiao do picker).
    noise = "assistant: passos:\n  1. abrir\n  2. fechar\n❯ \n"
    assert mp.parse_model_rows(noise) == []


# ── parse do esforco atual ───────────────────────────────────────────────────
def test_parse_current_effort_opus_is_xhigh():
    assert mp.parse_current_effort(PANE_OPUS) == "xhigh"


def test_parse_current_effort_haiku_not_supported_is_none():
    assert mp.parse_current_effort(PANE_HAIKU) is None


def test_parse_current_effort_handles_default_suffix():
    pane = PANE_OPUS.replace("◉ xHigh effort ←/→ to adjust", "● High effort (default) ←/→ to adjust")
    assert mp.parse_current_effort(pane) == "high"


# ── contagem de passos (modelo) ──────────────────────────────────────────────
def test_model_nav_steps_from_opus_cursor():
    rows = mp.parse_model_rows(PANE_OPUS)  # cursor na linha 2 (opus)
    assert mp.model_nav_steps(rows, "haiku") == 2  # Down 2
    assert mp.model_nav_steps(rows, "sonnet") == 1  # Down 1
    assert mp.model_nav_steps(rows, "default") == -1  # Up 1
    assert mp.model_nav_steps(rows, "opus") == 0  # ja esta


def test_model_nav_steps_unknown_target_raises():
    rows = mp.parse_model_rows(PANE_OPUS)
    with pytest.raises(ValueError):
        mp.model_nav_steps(rows, "gpt")


def test_model_nav_steps_offscreen_target_uses_fallback_number():
    # Cursor numa linha alta (5) e o alvo "default" (linha 1) fora da viewport -> fallback.
    rows = [
        {"number": 5, "keyword": "opus", "label": "Opus ✔", "cursor": True, "active": True},
        {"number": 4, "keyword": "haiku", "label": "Haiku", "cursor": False, "active": False},
    ]
    assert mp.model_nav_steps(rows, "default") == -4  # 1 - 5


# ── parse da linha de resultado ──────────────────────────────────────────────
def test_parse_result_line_session_only():
    pane = "  ⎿  Set model to Sonnet 4.6 for this session only with high effort\n❯ \n"
    assert mp.parse_result_line(pane) == "Set model to Sonnet 4.6 for this session only with high effort"


def test_parse_result_line_default():
    pane = "  ⎿  Set model to Opus 4.8 and saved as your default for new sessions\n❯ \n"
    assert "saved as your default" in mp.parse_result_line(pane)


def test_parse_result_line_absent():
    assert mp.parse_result_line("❯ \nsem resultado\n") is None


# ── driver (IO mockado): replay da sequencia de teclas ───────────────────────
def _pane_with_effort(level_word: str) -> str:
    # Troca o marcador de esforco do fixture Opus por outro nivel (pra simular o Right).
    return PANE_OPUS.replace("◉ xHigh effort", f"◉ {level_word} effort")


def test_set_model_effort_session_navigates_and_presses_s():
    # Alvo: Sonnet (linha 3) a partir do cursor em Opus (linha 2) => Down 1, depois `s`.
    result_pane = "❯ \n  ⎿  Set model to Sonnet 4.6 for this session only with xhigh effort\n"
    panes = [PANE_OPUS, _pane_with_effort("xHigh"), result_pane]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=[PANE_IDLE, *panes]), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().set_model_effort("cc", model="sonnet", scope="session")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys[:2] == ["/model", "Enter"]  # abre o picker
    assert "Down" in keys and keys.count("Down") == 1  # navega 1 linha (sem teclas de numero)
    assert keys[-1] == "s"  # confirma SO na sessao
    assert "session only" in out["result"]


def test_set_model_effort_default_presses_enter():
    result_pane = "❯ \n  ⎿  Set model to Opus 4.8 and saved as your default for new sessions\n"
    # capturas: abrir, pos-navegacao (opus = 0 passos, mas captura assim mesmo), pos-confirmar
    panes = [PANE_OPUS, PANE_OPUS, result_pane]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=[PANE_IDLE, *panes]), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        TerminalInput().set_model_effort("cc", model="opus", scope="default")
    keys = [c.args[1] for c in sk.call_args_list]
    # opus ja e o modelo atual -> sem Down/Up; confirma com Enter (default). 2 Enters: abrir + confirmar.
    assert "Down" not in keys and "Up" not in keys
    assert keys[-1] == "Enter"


def test_set_model_effort_adjusts_effort_with_right():
    # Esforco-so: xHigh -> max (1 Right). Sem modelo: cursor fica na linha atual.
    panes = [
        PANE_OPUS,  # abre: esforco xHigh
        _pane_with_effort("Max"),  # apos 1 Right
        "❯ \n  ⎿  Set model to Opus 4.8 for this session only with max effort\n",
    ]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=[PANE_IDLE, *panes]), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        TerminalInput().set_model_effort("cc", effort="max", scope="session")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys.count("Right") == 1
    assert keys[-1] == "s"


# Pane real do follow-up condicional disparado ao confirmar uma troca de effort (cache re-read).
EFFORT_CONFIRM_PANE = (
    "   Change effort level?\n"
    "   Your next response will be slower and use more tokens\n"
    "\n"
    "   This conversation is cached for the current effort level. Switching to max means the\n"
    "   full history gets re-read on your next message.\n"
    "\n"
    "   ❯ 1. Yes, switch to max\n"
    "     2. No, go back\n"
)


def test_effort_confirm_open_detects_dialog():
    assert mp.effort_confirm_open(EFFORT_CONFIRM_PANE) is True
    assert mp.effort_confirm_open(PANE_OPUS) is False  # picker normal != dialog de confirmacao


def test_set_model_effort_reports_pending_confirm_on_dialog():
    # xHigh -> max (1 Right); apos `s`, o follow-up "Change effort level?" aparece -> NAO auto-
    # confirma: retorna pending_confirm pro usuario decidir via OptionButtons. Ultimo toque = `s`.
    panes = [PANE_OPUS, _pane_with_effort("Max"), EFFORT_CONFIRM_PANE]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=[PANE_IDLE, *panes]), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().set_model_effort("cc", effort="max", scope="session")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys[-1] == "s"  # confirmou a sessao; nao tocou no menu de follow-up
    assert "Enter" not in keys[-2:]  # nao auto-confirmou o "Yes"
    assert out["pending_confirm"] == "max"
    assert out["result"] is None


def test_set_model_effort_aborts_when_picker_never_opens():
    not_open = "❯ \nsem picker aqui\n"
    with patch.object(terminal_input.tmux, "capture_pane", return_value=not_open), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        with pytest.raises(mp.PickerError) as ei:
            TerminalInput().set_model_effort("cc", model="sonnet")
    assert ei.value.status == 409
    assert sk.call_args_list[-1] == call("cc", "Escape")  # Esc pra nao deixar preso


def test_set_model_effort_rejects_model_absent_from_picker():
    # Quem diz o que existe e o PICKER lido ao vivo, nao uma lista chumbada: um nome que nao esta
    # nas linhas abre o picker, falha na navegacao e sai com Esc (409), sem confirmar nada.
    panes = [PANE_IDLE, *([PANE_OPUS] * 4)]  # guard le o chat parado; depois o picker aberto
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        with pytest.raises(mp.PickerError) as ei:
            TerminalInput().set_model_effort("cc", model="gpt")
    assert ei.value.status == 409
    assert sk.call_args_list[-1] == call("cc", "Escape")


def test_set_model_effort_rejects_malformed_model():
    # Formato invalido (vira comparacao de keyword, mas nao e uma palavra) ainda cai antes de tocar
    # no terminal.
    with pytest.raises(ValueError):
        TerminalInput().set_model_effort("cc", model="opus; rm -rf /")


def test_set_model_effort_requires_model_or_effort():
    with pytest.raises(ValueError):
        TerminalInput().set_model_effort("cc")


# ── lista de modelos lida do picker (nada chumbado no codigo) ────────────────
# Pane real capturado em 31/07/2026 (claude 2.1.220, conta Anthropic): 5 linhas, com o Fable no
# meio. E o fixture que prova o bug que motivou a mudanca — a lista de 4 chumbada no front nao
# mostrava o Fable e ainda dava a Sonnet/Haiku um numero de linha errado.
PANE_FABLE = (FIX / "pane_model_picker_fable.txt").read_text()


def test_parse_model_rows_reads_fable_and_descriptions():
    rows = mp.parse_model_rows(PANE_FABLE)
    assert [r["keyword"] for r in rows] == ["default", "opus", "fable", "sonnet", "haiku"]
    assert [r["number"] for r in rows] == [1, 2, 3, 4, 5]
    fable = rows[2]
    assert fable["name"] == "Fable"
    assert fable["desc"].startswith("Fable 5 ·")
    # o rotulo da linha ativa perde o ✔ no `name` (o ✔ vira a flag `active`)
    assert rows[1]["name"] == "Opus (1M context)" and rows[1]["active"] is True
    assert rows[0]["name"] == "Default"  # "(recommended)" sai do nome exibido


def test_list_model_options_reads_rows_and_closes_with_escape():
    panes = [PANE_IDLE, PANE_FABLE]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().list_model_options("cc")
    assert [m["keyword"] for m in out["models"]] == ["default", "opus", "fable", "sonnet", "haiku"]
    assert out["effort"] == "high"
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys[:2] == ["/model", "Enter"]
    assert keys[-1] == "Escape"      # so LE: fecha sem confirmar nada
    assert "s" not in keys and keys.count("Enter") == 1


def test_list_model_options_refuses_while_working():
    # Com o Claude trabalhando, digitar "/model" nao vira comando: cai no input e o Enter o
    # ENFILEIRA como mensagem. O que prova "vivo" e a ANIMACAO — o texto do spinner muda entre as
    # duas capturas.
    panes = ["✻ Crunched for 24s\n❯ \n", "✻ Crunched for 26s\n❯ \n"]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        with pytest.raises(mp.PickerError) as ei:
            TerminalInput().list_model_options("cc")
    assert ei.value.status == 409
    assert sk.call_args_list == []   # nao digitou NADA


def test_list_model_options_aceita_marcador_de_turno_congelado():
    # O bug medido: sessao que ACABOU de terminar fica com "✻ Crunched for 24s" na tela. Uma
    # captura so nao distingue isso de spinner vivo (esta na docstring do state.classify) e a
    # troca de modelo era recusada com "esta trabalhando" numa sessao parada.
    congelado = "✻ Crunched for 24s\n❯ \n"
    panes = [congelado, congelado, PANE_FABLE]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(
        terminal_input.tmux, "has_session", return_value=True
    ), patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().list_model_options("cc")
    assert [m["keyword"] for m in out["models"]][:2] == ["default", "opus"]
    assert [c.args[1] for c in sk.call_args_list][:2] == ["/model", "Enter"]


# ── troca de modelo numa sessao de motor: `/model <id>`, sem picker ──────────
def test_set_engine_model_types_command_and_reads_result():
    result_pane = "❯ \n  ⎿  Set model to kimi-for-coding and saved as your default for new sessions\n"
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=[PANE_IDLE, result_pane]), \
         patch.object(terminal_input, "send_keys") as sk, \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().set_engine_model("cc", "kimi-for-coding")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys == ["/model kimi-for-coding", "Enter"]
    assert out["ok"] is True and "kimi-for-coding" in out["result"]


def test_set_engine_model_aborts_when_command_opens_picker():
    # Argumento nao aceito -> o /model abre o picker interativo. Fecha e falha, em vez de deixar o
    # overlay preso e reportar sucesso sobre um no-op.
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=[PANE_IDLE, PANE_FABLE]), \
         patch.object(terminal_input, "send_keys") as sk, \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"):
        with pytest.raises(mp.PickerError) as ei:
            TerminalInput().set_engine_model("cc", "modelo-que-nao-existe")
    assert ei.value.status == 409
    assert sk.call_args_list[-1] == call("cc", "Escape")


def test_set_engine_model_rejects_argument_with_space():
    with pytest.raises(ValueError):
        TerminalInput().set_engine_model("cc", "k3 --dangerously")


# ── achados do review: confirmacao velha, e lock por sessao ──────────────────
def test_result_model_extrai_o_token_e_nao_casa_por_prefixo():
    # `k3` e prefixo de `k3-256k`: substring casaria na linha da troca ANTERIOR.
    velha = "Set model to k3-256k and saved as your default for new sessions"
    assert "k3" in velha                      # e por isso que substring nao serve
    assert mp.result_model(velha) == "k3-256k"
    assert mp.result_model("Set model to k3 and saved as your default") == "k3"
    assert mp.result_model("Kept model as k3") is None   # so a linha de troca carrega o id
    assert mp.result_model(None) is None


def test_set_engine_model_ignora_a_confirmacao_da_troca_anterior():
    # Cenario real: sessao estava em k3-256k, usuario pede k3. A linha velha continua na tela e a
    # nova demora ~1s. Sem comparar o token, a primeira leitura reportava sucesso na hora — sobre a
    # mensagem da troca passada, antes de esta acontecer.
    velha = "❯ \n  ⎿  Set model to k3-256k and saved as your default for new sessions\n"
    nova = velha + "  ⎿  Set model to k3 and saved as your default for new sessions\n"
    with patch.object(terminal_input.tmux, "capture_pane",
                      side_effect=[PANE_IDLE, velha, velha, nova]), \
         patch.object(terminal_input, "send_keys"), \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().set_engine_model("cc", "k3")
    assert mp.result_model(out["result"]) == "k3"   # a confirmacao aceita e a NOVA


def test_drivers_do_model_pegam_o_lock_da_sessao():
    # Dois toques concorrentes digitavam no MESMO tty: o Esc de um fechava o picker que o outro
    # estava navegando. O lock por sessao ja existia (send_prompt/send_pi_commands); os drivers do
    # /model precisam pedir o MESMO — o `_require_drivable` nao cobre isso, ele ve spinner e nao ve
    # outro driver em voo.
    ti = TerminalInput()
    resultado = "❯ \n  ⎿  Set model to k3 and saved as your default for new sessions\n"
    chamadas = [
        (lambda: ti.list_model_options("cc"), [PANE_IDLE, PANE_FABLE]),
        (lambda: ti.set_engine_model("cc", "k3"), [PANE_IDLE, resultado]),
        (lambda: ti.set_model_effort("cc", model="opus"),
         [PANE_IDLE, PANE_FABLE, PANE_FABLE,
          "❯ \n  ⎿  Set model to Opus 5 for this session only with high effort\n"]),
    ]
    for chamada, panes in chamadas:
        pedidos: list[str] = []
        real = terminal_input._send_lock

        def espiao(nome, _pedidos=pedidos, _real=real):
            _pedidos.append(nome)
            return _real(nome)

        with patch.object(terminal_input, "_send_lock", espiao), \
             patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), \
             patch.object(terminal_input, "send_keys"), \
             patch.object(terminal_input.tmux, "has_session", return_value=True), \
             patch.object(terminal_input.time, "sleep"):
            chamada()
        assert pedidos == ["cc"], f"driver nao pediu o lock da sessao (pediu {pedidos})"


def test_recusa_antes_de_digitar_e_uma_excecao_distinta():
    # A rota usa isso pra NAO esperar os ~3.6s da escrita do settings.json quando o terminal ficou
    # intocado — senao todo "sessao ocupada" demora 3.6s pra virar erro na tela.
    working = ["✻ Crunched for 24s\n❯ \n", "✻ Crunched for 26s\n❯ \n"]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=working), \
         patch.object(terminal_input, "send_keys"), \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"):
        with pytest.raises(TerminalInput.NaoDigitou):
            TerminalInput().set_engine_model("cc", "k3")


def test_abrir_picker_insiste_na_leitura_antes_de_arriscar_um_2o_enter():
    # O 2o Enter existe pro caso do autocomplete engolir o 1o. Mas se o picker JA abriu e a captura
    # pegou o redraw pela metade, esse Enter confirma a linha sob o cursor COMO DEFAULT — trocando o
    # modelo padrao do usuario num caminho que era pra ser so leitura (a folha busca a lista sozinha
    # ao abrir). Entao: relê primeiro, e so manda o 2o Enter se o picker realmente nao veio.
    meio_redraw = "❯ \n"                      # captura pegou a tela em transicao
    panes = [PANE_IDLE, meio_redraw, PANE_FABLE]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), \
         patch.object(terminal_input, "send_keys") as sk, \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().list_model_options("cc")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys.count("Enter") == 1           # NENHUM Enter extra
    assert len(out["models"]) == 5


def test_espera_o_picker_INTEIRO_antes_de_ler_a_lista():
    # Medido numa sessao real: no instante em que o titulo aparece, as linhas ainda estao sendo
    # pintadas — a leitura devolvia 4 modelos, sem o Haiku (a ultima linha). O rodape vem depois das
    # linhas, entao ele e a prova de que a lista esta completa.
    meio = PANE_FABLE.replace("     5. Haiku", "").replace(
        "   Enter to set as default · s to use this session only · Esc to cancel", "")
    assert mp.picker_open(meio) and not mp.picker_desenhado(meio)
    panes = [PANE_IDLE, meio, PANE_FABLE]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), \
         patch.object(terminal_input, "send_keys") as sk, \
         patch.object(terminal_input.tmux, "has_session", return_value=True), \
         patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().list_model_options("cc")
    assert [m["keyword"] for m in out["models"]] == ["default", "opus", "fable", "sonnet", "haiku"]
    assert [c.args[1] for c in sk.call_args_list].count("Enter") == 1
