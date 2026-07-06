import re
import threading
import time

from app import model_picker as mp
from app import tmux
from app.pqueue import PromptQueue, _transcript_start_ts
from app.state import classify, is_overlay
from app.tmux import send_keys

# Tempos de acomodacao do TUI entre toque e leitura do pane (o picker redesenha em overlay).
_SETTLE = 0.3  # apos uma tecla de navegacao
_OPEN_SETTLE = 0.7  # apos abrir o picker / confirmar (precisa redesenhar/commitar o resultado)
_NAV_GAP = 0.12  # entre toques Up/Down em rajada
_SLASH_SETTLE = 0.3  # apos digitar "/cmd": deixa o menu de autocomplete renderizar antes do Enter
_SUBMIT_SETTLE = 0.2  # entre o texto livre e o Enter: claude detecta input rapido como paste e engole o Enter


def _capture(name: str) -> str:
    """Lê o pane atual da sessão tmux (wrapper em módulo para permitir patch nos testes)."""
    return tmux.capture_pane(name)


# Marcas do rodapé do Claude Code quando o input já está VIVO (TUI interativo). Durante o BOOT
# (logo + carregamento) elas ainda não aparecem.
_READY_MARKERS = ("bypass permissions", "? for shortcuts", "for shortcuts")


def _wait_input_ready(name: str, timeout: float = 12.0) -> bool:
    """Espera o TUI do Claude ficar interativo antes de enviar. BUG: msg mandada logo após criar a
    sessão (claude ainda bootando, TUI não aceita teclas) era ENGOLIDA -> sumia (ficava só na fila
    como bubble fantasma, claude nunca recebia). Sessão já pronta -> retorna na 1ª leitura (sem
    latência). Timeout -> retorna False e envia mesmo assim (não piora o caso de hoje)."""
    deadline = time.monotonic() + timeout
    while True:
        if any(m in _capture(name) for m in _READY_MARKERS):
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.2)


def deliverable(name: str) -> bool:
    # Pode entregar texto livre AGORA? False se a sessao morreu (defer p/ recriacao, sem queimar 12s no
    # _wait_input_ready) ou se ha overlay/menu aberto (digitar as cegas navegaria o menu errado). Erro
    # de captura (sessao viva, pane ileg.) -> True: degrada pro envio de hoje, sem regressao.
    if not tmux.has_session(name):
        return False
    try:
        pane = _capture(name)
    except Exception:
        return True
    state, _, _, _ = classify(pane)
    return state != "awaiting_input" and not is_overlay(pane)


# Lock POR SESSAO serializando o send_prompt: dois /input quase simultaneos (ou /input + drain)
# rodavam em threads digitando no MESMO tty — o texto de B aterrissava na janela de settle de A e o
# Enter de A submetia os dois CONCATENADOS. setdefault e atomico no CPython (pior caso: Lock orfao).
_send_locks: dict[str, threading.Lock] = {}


def _send_lock(name: str) -> threading.Lock:
    return _send_locks.setdefault(name, threading.Lock())


def drain(name: str, jsonl: str) -> int:
    """Entrega ao tty as entradas pendentes (delivered=False) quando o pane volta a aceitar texto.
    Retorna quantas entregou. claim-1-envia-1: um crash entre o claim e o envio deixa NO MAXIMO 1
    entrada 'stranded', nao o lote, e recheca o overlay (via send_prompt) a cada iteracao."""
    q = PromptQueue(name)
    # ECC: cheap-check SEM subprocess primeiro — a maioria das reconexoes nao tem pendencia; sem isto,
    # todo (re)connect dispararia um capture-pane atoa (pressao no threadpool em rajada de mobile).
    if not any(e.get("delivered") is False for e in q.load()):
        return 0
    start_ts = _transcript_start_ts(jsonl)   # poda entradas de sessao antiga (pre-/clear)
    # Orfas de sessao anterior: nunca mais casam nem drenam — remove (senao o cheap-check acima
    # fica quente pra sempre e o lixo acumula ate o cap).
    q.prune_before(start_ts)
    ti = TerminalInput()
    sent = 0
    while True:
        claimed = q.claim_undelivered(min_ts=start_ts, limit=1)
        if not claimed:
            return sent
        entry = claimed[0]
        try:
            result = ti.send_prompt(name, entry["text"])
        except Exception:
            # Falha POS-gate (tty caiu no meio): pode ter emitido tecla -> at-most-once, NAO reverte.
            # ponytail: stranded-mas-visivel (a bubble queued- segue aparecendo, display ignora delivered);
            # upgrade: render distinto / re-drain confirmado-por-transcript se virar reclamacao real.
            return sent
        if result == "deferred":
            # send_prompt NAO tocou a TUI (overlay reabriu entre claim e envio): reverte (provadamente
            # pre-envio) e para — espera o proximo idle. Revert pode falhar (disco): nesse caso a entrada
            # fica delivered=True (stranded-mas-VISIVEL como bubble queued-) -> nao re-dreva, mas nao some;
            # nunca propaga (drain roda fire-and-forget no to_thread). delivered=True = "send_keys chamado",
            # nao "Claude recebeu" (tmux engole erro de envio) -> a bubble visivel e a unica garantia.
            try:
                q.set_delivered(entry["id"], False)
            except OSError:
                pass
            return sent
        sent += 1


class DriveError(RuntimeError):
    """Falha ao dirigir o picker (nav nao convergiu / review mismatch / picker preso). NADA foi
    submetido e NENHUM Escape foi mandado — o picker segue aberto; o caller (api /answer) decide o
    fallback (Escape + resposta por texto). Nao herda ValueError de proposito: ValueError = input
    invalido pre-TUI (409), DriveError = TUI nao cooperou (fallback)."""


# Linha destacada do picker: "❯ 3. OPT-TWO" -> 3 (1-based). Numerico de proposito: robusto a label
# longo/quebrado em multiplas linhas, que um match por texto erraria.
_CURSOR_ROW = re.compile(r"❯\s*(\d+)\.")


def _cursor_row(screen: str) -> int | None:
    m = _CURSOR_ROW.search(screen)
    return int(m.group(1)) if m else None


def _review_matches(screen: str, answers: list[dict]) -> bool:
    # Cada pergunta no review vira uma linha "→ <labels por ', '>". Compara por TOKEN exato (nao
    # substring) pra um label curto nao casar dentro de outra palavra.
    # ponytail: split por ',' assume que label nao contem ',' (labels do AskUserQuestion sao frases curtas)
    arrow_tokens = [
        {p.strip() for p in line.split("→", 1)[1].split(",")}
        for line in screen.splitlines() if line.strip().startswith("→")
    ]
    for a in answers:
        for lbl in a.get("labels", []):
            if not any(lbl in toks for toks in arrow_tokens):
                return False
    return True


def _validate(answers: list[dict]) -> None:
    # Valida tudo ANTES de mandar tecla: se algo falta, o TUI nunca e tocado e o ValueError vira 409
    # limpo (em vez de 500 + TUI travado no meio do caminho).
    for a in answers:
        kind = a.get("kind")
        if kind == "text":
            v = a.get("value")
            if v is None:
                raise ValueError("value required for text kind")
            if any(ord(c) < 32 and c not in "\t" for c in v):
                raise ValueError("control characters not allowed")
            if a.get("type_index") is None:
                raise ValueError("type_index required for text kind")
        elif kind == "chat":
            if a.get("chat_index") is None:
                raise ValueError("chat_index required for chat kind")
        elif kind == "option":
            if not a.get("indices"):
                raise ValueError("indices required for option kind")
        else:
            raise ValueError(f"unknown answer kind: {kind!r}")


def answer_questions(name: str, answers: list[dict]) -> None:
    """Dirige o prompt tabbed AskUserQuestion do estado INICIAL (cursor aba1/opt0) e CONFERE no Review
    antes do Submit. Input invalido -> ValueError pre-TUI (API -> 409). Drive falhou (nav nao
    convergiu / mismatch / preso) -> DriveError SEM submeter e SEM Escape — o caller manda Escape e
    reenvia a resposta como texto (fallback), pra o Escape solto nao virar "user declined".
    single = Down*idx + Enter (auto-avanca); multi = (Down ate idx + Space) por opcao, depois Right;
    texto = Down ate 'Type something' + Enter + digita + Enter; chat = Down ate 'Chat about this' + Enter."""
    _validate(answers)  # valida ANTES de tocar no TUI; loop abaixo assume input valido

    def key(k: str) -> None:
        send_keys(name, k)
        time.sleep(_SETTLE)

    for a in answers:
        kind = a.get("kind")
        if kind == "option" and not a.get("multi"):
            # single-select: desce ate o indice e Enter (TUI auto-avanca pro proximo tab)
            for _ in range(a["indices"][0]):
                key("Down")
            # Guard PRE-Enter em MALHA FECHADA: numa pergunta UNICA o Enter ja SUBMETE (nao ha tela
            # de Review depois p/ pegar drift). Um Down engolido no redraw do overlay submetia a
            # opcao errada calado. Le a linha REAL do cursor e CORRIGE (Down/Up + re-le, ate 3x) —
            # tecla engolida vira ruido auto-corrigido, nao erro. Cursor abre na linha 1 (indice 0),
            # logo esperado = indice+1. Linha ilegivel -> segue como hoje (guard so age se leu).
            # Nao convergiu -> DriveError SEM Escape (caller faz Escape + fallback por texto).
            expected = a["indices"][0] + 1
            row = _cursor_row(_capture(name))
            for _ in range(3):
                if row is None or row == expected:
                    break
                for _ in range(abs(expected - row)):
                    key("Down" if expected > row else "Up")
                row = _cursor_row(_capture(name))
            if row is not None and row != expected:
                raise DriveError(f"nav drift nao corrigido — cursor na linha {row}, esperava {expected}; nao submetido")
            key("Enter")
        elif kind == "option":
            # multi-select: para cada opcao (em ordem crescente) desce ate ela e Space; depois Right
            cur = 0
            for idx in sorted(a["indices"]):
                for _ in range(idx - cur):
                    key("Down")
                cur = idx
                key("Space")
            key("Right")
        elif kind == "text":
            # texto livre: desce ate 'Type something', Enter abre campo, digita valor, Enter submete
            for _ in range(a["type_index"]):
                key("Down")
            key("Enter")
            send_keys(name, a["value"], literal=True)  # control-char ja validado em _validate
            time.sleep(_SETTLE)
            key("Enter")
        elif kind == "chat":
            # 'Chat about this': desce ate o indice e Enter
            for _ in range(a["chat_index"]):
                key("Down")
            key("Enter")

    # Passo final depende do shape do TUI:
    #  - MULTIPLAS perguntas -> tela "Review your answers / Submit answers": confere e da Enter p/ submeter.
    #  - UNICA pergunta -> NAO ha review; o Enter da selecao ja submeteu. Sucesso, sem Escape (mandar
    #    Escape aqui interrompia o Claude que ja recebeu a resposta -> bug do "aceitou mas deu ruim").
    #  - Picker ainda aberto sem review (algo travou) -> Escape e erro, nunca submete as cegas.
    screen = _capture(name)
    if "Submit answers" in screen:
        if not _review_matches(screen, answers):
            raise DriveError("review mismatch — nao submetido")
        key("Enter")
    elif "Esc to cancel" in screen:
        raise DriveError("picker preso sem tela de review — nao submetido")
    # senao: pergunta unica ja submeteu na selecao; nada a confirmar.


class TerminalInput:
    def send_prompt(self, name: str, text: str) -> str:
        # Validacao PRE-envio: input ruim nunca toca a TUI nem entra na fila. \n/\t ok; outros controles nao.
        if any(ord(c) < 32 and c not in "\t\n" for c in text):
            raise ValueError("control characters not allowed in prompt")
        # Serializa por sessao (gate + digitacao + Enter como unidade): sem o lock, envios
        # concorrentes intercalavam teclas no mesmo tty e as mensagens saiam concatenadas.
        with _send_lock(name):
            # Gate de entregabilidade (chokepoint UNICO p/ texto livre — /input e drain passam por
            # aqui): nao digitar as cegas num overlay (AskUserQuestion/picker), as teclas o
            # corromperiam. Sem pane entregavel agora, devolve "deferred" SEM tocar a TUI; o caller
            # enfileira pendente e o drain entrega quando o overlay fechar / a sessao voltar.
            if not deliverable(name):
                return "deferred"
            # Não enviar pra um TUI ainda bootando: as teclas seriam engolidas e a msg sumiria (core
            # bug — msg mandada logo após criar a sessão nunca chegava no claude).
            _wait_input_ready(name)
            if "\n" in text:
                tmux.paste_text(name, text)
                time.sleep(0.05)  # deixa a TUI acomodar o paste antes do Enter submeter
                send_keys(name, "Enter")
            elif text.lstrip().startswith("/"):
                # Slash command: ao digitar "/..." o Claude Code abre um menu de autocomplete. Sem dar
                # tempo do menu renderizar, o Enter corre com o redraw e e ENGOLIDO pelo menu (o comando
                # fica digitado mas NAO executa -> "o slash nao chega no terminal"). Espera o menu
                # acomodar, Enter pra executar; um 2o Enter cobre o caso do 1o so ter selecionado a
                # sugestao (o comando ja rodou e o prompt esta vazio -> o 2o Enter e no-op inofensivo).
                send_keys(name, text, literal=True)
                time.sleep(_SLASH_SETTLE)
                send_keys(name, "Enter")
                time.sleep(_SLASH_SETTLE)
                send_keys(name, "Enter")
            else:
                send_keys(name, text, literal=True)
                # Settle ANTES do Enter: sem isto o Enter corria a ingestao do texto e o claude (que
                # detecta input rapido como paste) tratava o Enter como parte do conteudo -> o texto
                # ficava no input SEM submeter (usuario tinha que reenviar). Espelha o gap multiline.
                # ponytail: settle fixo; se ainda escapar em device lento, upgrade = capturar o pane e
                # reenviar Enter se o input nao limpou.
                time.sleep(_SUBMIT_SETTLE)
                send_keys(name, "Enter")
            return "sent"

    # Teclas de navegacao liberadas pro espelho do pane (TerminalMirror dirige overlays so-TUI:
    # /status, /config, /help, pickers). Allowlist (nao texto livre) pra so passar navegacao -> nada
    # de control chars arbitrarios na TUI. Valor = nome de tecla do tmux send-keys (PPage/NPage =
    # PageUp/PageDown; BTab = Shift-Tab).
    _NAV_KEYS = {
        "Up": "Up", "Down": "Down", "Left": "Left", "Right": "Right",
        "Enter": "Enter", "Escape": "Escape", "Tab": "Tab", "BTab": "BTab",
        "PageUp": "PPage", "PageDown": "NPage", "Space": "Space",
    }

    def send_key(self, name: str, key: str) -> None:
        # Manda UMA tecla de navegacao (allowlist) pro pane. Usado pelo espelho do pane.
        tmux_key = self._NAV_KEYS.get(key)
        if tmux_key is None:
            raise ValueError(f"key not allowed: {key!r}")
        send_keys(name, tmux_key)

    # Terminal INTERATIVO (so desktop): alem da navegacao, edicao de linha + control-chars de
    # shell/TUI. Texto livre vai literal (send_text); teclas nomeadas por esta allowlist.
    _TERM_KEYS = {
        **_NAV_KEYS,
        "Backspace": "BSpace", "Delete": "DC", "Home": "Home", "End": "End",
        "C-c": "C-c", "C-d": "C-d", "C-r": "C-r", "C-u": "C-u", "C-k": "C-k",
        "C-w": "C-w", "C-a": "C-a", "C-e": "C-e", "C-l": "C-l", "C-z": "C-z",
        "C-p": "C-p", "C-n": "C-n", "C-b": "C-b", "C-f": "C-f", "C-g": "C-g",
    }

    def send_text(self, name: str, text: str) -> None:
        # Texto digitado no terminal desktop. Literal -> tmux nao interpreta como nome de tecla.
        if text:
            send_keys(name, text, literal=True)

    def send_term_key(self, name: str, key: str) -> None:
        tmux_key = self._TERM_KEYS.get(key)
        if tmux_key is None:
            raise ValueError(f"key not allowed: {key!r}")
        send_keys(name, tmux_key)

    def select(self, name: str, option: int) -> None:
        if option < 1:
            raise ValueError("option must be >= 1")
        for _ in range(option - 1):
            send_keys(name, "Down")
        send_keys(name, "Enter")

    def interrupt(self, name: str, clear: bool = False) -> None:
        # Esc UNICO = interrompe o Claude MAS mantem o texto enfileirado no input (doc oficial). Por isso
        # o proximo envio digitava EM CIMA do residuo -> concatenava. clear=True manda um 2o Esc: com o
        # input nao-vazio (garantido pelo caller — so passa clear quando havia msg pendente) o Esc-Esc
        # limpa o draft. NUNCA mandar o 2o Esc as cegas: input vazio + Esc-Esc abre o menu de rewind.
        send_keys(name, "Escape")
        if clear:
            time.sleep(_SETTLE)  # deixa o interrupt assentar e o texto voltar pro input antes de limpar
            send_keys(name, "Escape")

    def set_model_effort(
        self,
        name: str,
        model: str | None = None,
        effort: str | None = None,
        scope: str = "session",
    ) -> dict:
        """Aplica modelo e/ou esforco dirigindo o picker interativo do `/model`.

        scope='session' aperta `s` (so a sessao atual); scope='default' aperta Enter (salva
        como default). Le o pane a cada passo (drive nao-cego). Em qualquer falha de parse,
        manda Esc e levanta PickerError pra o picker nunca ficar preso.
        """
        if scope not in ("session", "default"):
            raise ValueError("scope must be 'session' or 'default'")
        model_kw = model.strip().lower() if model else None
        effort_kw = effort.strip().lower() if effort else None
        if model_kw is None and effort_kw is None:
            raise ValueError("must provide model or effort")
        if model_kw and model_kw not in mp.MODEL_ORDER:
            raise ValueError(f"unknown model {model!r}")
        if effort_kw and effort_kw not in mp.EFFORT_ORDER:
            raise ValueError(f"unknown effort {effort!r}")

        # 1. Abre o picker: "/model" + Enter. Se o autocomplete engolir o 1o Enter, um 2o
        #    submete (guardado por picker_open pra nunca mandar Enter num picker ja aberto,
        #    o que confirmaria como default).
        send_keys(name, "/model", literal=True)
        time.sleep(_SETTLE)
        send_keys(name, "Enter")
        time.sleep(_OPEN_SETTLE)
        pane = tmux.capture_pane(name)
        if not mp.picker_open(pane):
            send_keys(name, "Enter")
            time.sleep(_OPEN_SETTLE)
            pane = tmux.capture_pane(name)
        if not mp.picker_open(pane):
            self._abort(name)
            raise mp.PickerError(409, "model picker did not open")

        # 2. Navega o modelo (Up/Down). O cursor abre sobre o modelo atual; calculamos os
        #    passos a partir do pane limpo do open (evita reler o cursor, que pode fantasmar
        #    no redraw). Numeros so com setas -- nunca teclas de numero (= confirma default).
        if model_kw:
            rows = mp.parse_model_rows(pane)
            try:
                steps = mp.model_nav_steps(rows, model_kw)
            except ValueError as e:
                self._abort(name)
                raise mp.PickerError(409, f"cannot navigate model picker: {e}")
            key = "Down" if steps > 0 else "Up"
            for _ in range(abs(steps)):
                send_keys(name, key)
                time.sleep(_NAV_GAP)
            time.sleep(_SETTLE)
            pane = tmux.capture_pane(name)

        # 3. Ajusta o esforco (Left/Right, ciclico). Le o marcador a cada Right e para no
        #    alvo; se der a volta inteira sem casar, o nivel nao existe pra esse modelo ->
        #    fica no atual (sem mexer) e segue (o modelo ja e o que importa).
        if effort_kw:
            current = mp.parse_current_effort(pane)
            if current is not None and current != effort_kw:
                start = current
                for _ in range(len(mp.EFFORT_ORDER) + 1):
                    send_keys(name, "Right")
                    time.sleep(_SETTLE)
                    pane = tmux.capture_pane(name)
                    current = mp.parse_current_effort(pane)
                    if current is None:  # redraw transiente -> tenta reler uma vez
                        time.sleep(_SETTLE)
                        pane = tmux.capture_pane(name)
                        current = mp.parse_current_effort(pane)
                    if current == effort_kw:
                        break
                    if current == start:  # ciclo completo: nivel indisponivel pra esse modelo
                        break

        # 4. Confirma: `s` = so a sessao; Enter = salva como default.
        send_keys(name, "s" if scope == "session" else "Enter")
        time.sleep(_OPEN_SETTLE)
        pane = tmux.capture_pane(name)

        # 4b. Trocar o effort pode disparar um follow-up CONDICIONAL "Change effort level?" (so
        #     quando ha cache a re-ler). Por design NAO confirmamos sozinhos: deixamos o menu pra
        #     o usuario decidir (o app o mostra como OptionButtons via state.classify). Reporta
        #     pending_confirm em vez de mascarar como ok aplicado -- o effort so pega quando o
        #     usuario tocar "Yes". (O picker ja fechou; nao mexer no menu.)
        if mp.effort_confirm_open(pane):
            return {"ok": True, "scope": scope, "pending_confirm": effort_kw, "result": None}

        if mp.picker_open(pane):
            self._abort(name)
            raise mp.PickerError(409, "model picker did not close after confirm")

        # 5. Verifica o resultado. Se pedimos sessao mas veio "default", a confirmacao errou
        #    -> expoe a falha em vez de mascarar. (O alvo pode disparar um "Switch model?" de
        #    follow-up; nesse caso nao ha linha de resultado ainda -- ok, o SSE cuida disso.)
        result = mp.parse_result_line(pane)
        if (
            scope == "session"
            and result
            and "session only" not in result.lower()
            and "default" in result.lower()
        ):
            raise mp.PickerError(409, f"expected session-only switch, got: {result}")
        return {"ok": True, "scope": scope, "result": result}

    def _abort(self, name: str) -> None:
        send_keys(name, "Escape")
