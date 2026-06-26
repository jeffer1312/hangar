import time

from app import model_picker as mp
from app import tmux
from app.tmux import send_keys

# Tempos de acomodacao do TUI entre toque e leitura do pane (o picker redesenha em overlay).
_SETTLE = 0.3  # apos uma tecla de navegacao
_OPEN_SETTLE = 0.7  # apos abrir o picker / confirmar (precisa redesenhar/commitar o resultado)
_NAV_GAP = 0.12  # entre toques Up/Down em rajada
_SLASH_SETTLE = 0.3  # apos digitar "/cmd": deixa o menu de autocomplete renderizar antes do Enter


class TerminalInput:
    def send_prompt(self, name: str, text: str) -> None:
        # Permite \n (multi-linha) e \t; rejeita os OUTROS controles (ESC etc. poderiam disparar acoes
        # na TUI). Multi-linha vai por bracketed paste (insere as quebras sem submeter); depois Enter.
        if any(ord(c) < 32 and c not in "\t\n" for c in text):
            raise ValueError("control characters not allowed in prompt")
        if "\n" in text:
            tmux.paste_text(name, text)
            time.sleep(0.05)  # deixa a TUI acomodar o paste antes do Enter submeter
            send_keys(name, "Enter")
        elif text.lstrip().startswith("/"):
            # Slash command: ao digitar "/..." o Claude Code abre um menu de autocomplete. Sem dar tempo
            # do menu renderizar, o Enter corre com o redraw e e ENGOLIDO pelo menu (o comando fica
            # digitado mas NAO executa -> "o slash nao chega no terminal"). Espera o menu acomodar, Enter
            # pra executar; um 2o Enter cobre o caso do 1o so ter selecionado a sugestao (o comando ja
            # rodou e o prompt esta vazio -> o 2o Enter e no-op inofensivo).
            send_keys(name, text, literal=True)
            time.sleep(_SLASH_SETTLE)
            send_keys(name, "Enter")
            time.sleep(_SLASH_SETTLE)
            send_keys(name, "Enter")
        else:
            send_keys(name, text, literal=True)
            send_keys(name, "Enter")

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

    def select(self, name: str, option: int) -> None:
        if option < 1:
            raise ValueError("option must be >= 1")
        for _ in range(option - 1):
            send_keys(name, "Down")
        send_keys(name, "Enter")

    def interrupt(self, name: str) -> None:
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
