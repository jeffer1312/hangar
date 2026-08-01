#!/usr/bin/env python3
"""Trava a resolução de janela do cp-panel-open, sem hyprland/tmux/kitty reais.

O bug que originou isto: com `kitty -1` (single-instance) TODAS as janelas do kitty compartilham
um pid, e o mapa pid->janela de valor único ficava com a última do `hyprctl clients`. Resultado:
clicar em qualquer sessão do painel focava sempre a MESMA janela, calado.

Rodar: python3 scripts/test-panel-open.py
"""
import contextlib
import importlib.util
import io
import sys
from pathlib import Path

CAMINHO = Path(__file__).with_name("cp-panel-open")


def carregar():
    """Importa o script (sem extensão .py) sem executar o main()."""
    spec = importlib.util.spec_from_loader("cp_panel_open", None)
    mod = importlib.util.module_from_spec(spec)
    src = CAMINHO.read_text().replace('if __name__ == "__main__":\n    sys.exit(main())\n', "")
    exec(compile(src, str(CAMINHO), "exec"), mod.__dict__)  # noqa: S102
    return mod


# Uma janela por sessão, todas no MESMO pid (kitty -1). Títulos = basename do cwd, como o rice faz.
JANELAS_KITTY1 = [
    {"pid": 63482, "class": "kitty", "title": "servicos-api", "address": "0xAAA"},
    {"pid": 63482, "class": "kitty", "title": "app-web", "address": "0xBBB"},
    {"pid": 63482, "class": "kitty", "title": "claude-cockpit", "address": "0xCCC"},
    {"pid": 63482, "class": "kitty", "title": "jefferson", "address": "0xDDD"},
]

# Ancestralidade: cliente tmux -> fish -> terminal.
ARVORE = {2324949: 2324733, 2324733: 63482, 615602: 615093, 615093: 63482,
          2562331: 71410, 71410: 63482, 2606971: 72172, 72172: 63482,
          900001: 900000, 900000: 71001}

CLIENTES = {"claude-cockpit-2": [2324949], "jefferson": [615602],
            "app-web": [2562331], "servicos-api": [2606971]}

TITULOS = {"claude-cockpit-2": "claude-cockpit", "jefferson": "jefferson",
           "app-web": "app-web", "servicos-api": "servicos-api"}


def montar(mod, janelas, clientes=None, titulos=None):
    mod.hypr_clients = lambda: janelas
    mod.ppid_of = lambda pid: ARVORE.get(pid, 0)
    mod.tmux_client_pids = lambda n: (clientes if clientes is not None else CLIENTES).get(n, [])
    mod.expected_title = lambda n: (titulos if titulos is not None else TITULOS).get(n)


def main() -> int:
    mod = carregar()

    # 1. O bug: 4 janelas num pid só, cada sessão tem que achar a SUA.
    montar(mod, JANELAS_KITTY1)
    esperado = {"servicos-api": "0xAAA", "app-web": "0xBBB",
                "claude-cockpit-2": "0xCCC", "jefferson": "0xDDD"}
    for sessao, addr in esperado.items():
        obtido = mod.find_window(sessao)
        assert obtido == addr, f"{sessao}: esperava {addr}, veio {obtido}"

    # 2. Um processo por janela (foot/alacritty/kitty sem -1): resolve por pid, sem olhar título.
    montar(mod, [{"pid": 71001, "class": "foot", "title": "titulo-que-nao-casa", "address": "0xEEE"}],
           clientes={"solo": [900001]}, titulos={"solo": "outra-coisa"})
    assert mod.find_window("solo") == "0xEEE", "pid único deve bastar, sem depender do título"

    # 3. Empate real (duas sessões no MESMO repo -> mesmo título): devolve None em vez de chutar.
    #    Abrir attach duplicado é visível; focar a janela errada é o bug silencioso que isto matou.
    empate = [
        {"pid": 63482, "class": "kitty", "title": "claude-cockpit", "address": "0xCCC"},
        {"pid": 63482, "class": "kitty", "title": "claude-cockpit", "address": "0xFFF"},
    ]
    montar(mod, empate)
    assert mod.find_window("claude-cockpit-2") is None, "empate de título não pode chutar janela"

    # 4. Sessão sem cliente attachado: nada a focar, e NÃO cai no título (janela de outra sessão).
    montar(mod, JANELAS_KITTY1, clientes={"sem-cliente": []})
    assert mod.find_window("sem-cliente") is None, "sem cliente attachado não há janela a focar"

    # 5. `set-titles-string` ausente/vazio (bloco do installer ainda não aplicado): sem título não
    #    dá pra desempatar — mas tem que DIZER isso. O sintoma (abre terminal novo) é igual ao do
    #    empate do caso 3, então sem a linha no stderr não há como distinguir as duas causas.
    montar(mod, JANELAS_KITTY1, titulos={})
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        obtido = mod.find_window("claude-cockpit-2")
    assert obtido is None, f"sem título esperado não pode escolher janela, veio {obtido}"
    assert "set-titles-string" in err.getvalue(), f"faltou diagnóstico no stderr: {err.getvalue()!r}"

    print("ok: 5 casos")
    return 0


if __name__ == "__main__":
    sys.exit(main())
