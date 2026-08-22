"""Os call sites do `tmp+rename` passam pelo `atomico.substituir` — inclusive os que escrevem
`tmp.replace(alvo)`, o metodo do `Path`.

O commit anterior converteu os 14 lugares escritos como `os.replace(tmp, alvo)` e deixou de fora 19
escritos como `tmp.replace(alvo)`. Nao e outra classe de bug: `Path.replace` E `os.replace` por
baixo (`pathlib` chama `os.replace(self, target)`), entao no Windows ele levanta o mesmo
PermissionError [WinError 5] quando alguem esta com o destino aberto so pra leitura — e ficaram de
fora justamente os sidecars que tem leitor concorrente por desenho (fila duravel, marcador lido por
hook em outro processo, cache de tarifas).

Os casos nao dependem do sistema de quem roda: o Windows e SIMULADO trocando `os.replace` por um
que falha as primeiras vezes. Como `pathlib` resolve `os.replace` no mesmo objeto de modulo, a
troca alcanca os dois jeitos de escrever — o codigo velho falha aqui no Linux tambem, que e a regra
do projeto.
"""
import ast
import json
import os
from pathlib import Path

import pytest

from app import atomico, hook_state, pqueue
from app.pqueue import PromptQueue

_APP = Path(atomico.__file__).parent


@pytest.fixture
def windows_com_leitor(monkeypatch):
    """Simula o Windows com o destino aberto por outro processo nas 2 primeiras tentativas.

    Patch em `os.replace` (o atributo do modulo), nao em `atomico.os.replace`: e o MESMO objeto, e
    so assim o `Path.replace` do codigo velho — que resolve `os.replace` dentro do `pathlib` —
    tambem sente a falha. Sem isso o teste passaria no codigo velho por acidente, ja que no Linux
    renomear por cima de arquivo aberto funciona.
    """
    real = os.replace
    estado = {"chamadas": 0}

    def falso(origem, destino):
        estado["chamadas"] += 1
        if estado["chamadas"] <= 2:
            raise PermissionError(5, "Acesso negado")
        return real(origem, destino)

    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    monkeypatch.setattr(os, "replace", falso)
    monkeypatch.setattr(atomico.time, "sleep", lambda _s: None)   # sem espera real no teste
    return estado


def _chamadas_de_rename(arquivo: Path) -> list[int]:
    """Linhas com `<algo>.replace(<um argumento posicional>)` — a assinatura do rename de arquivo.

    Um argumento posicional e so: `str.replace` exige dois, e `datetime.replace` vai de palavra-
    chave (`dt.replace(tzinfo=...)`). Entao esta forma e sempre `Path.replace`, e nunca as outras
    tres que existem no `app/`.
    """
    arvore = ast.parse(arquivo.read_text(encoding="utf-8"))
    return [n.lineno for n in ast.walk(arvore)
            if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)
            and n.func.attr == "replace" and len(n.args) == 1 and not n.keywords]


def test_nenhum_modulo_escreve_Path_replace():
    """A trava que cobre os 19 de uma vez — e o 20o que alguem escrever amanha.

    Um teste de comportamento por call site nao escala (e nao existiria pro proximo). Aqui o alvo e
    a FORMA: quem grava sidecar chama `atomico.substituir`, que e o unico lugar que sabe do Windows.
    """
    achados = {str(f.relative_to(_APP)): linhas
               for f in sorted(_APP.rglob("*.py"))
               if (linhas := _chamadas_de_rename(f))}
    assert achados == {}, f"use atomico.substituir nestes pontos: {achados}"


def test_a_varredura_acha_o_padrao_velho(tmp_path):
    """Contra-prova: sem isto, um erro no reconhecimento faria o teste de cima passar vazio."""
    velho = tmp_path / "velho.py"
    velho.write_text(
        "import os\n"
        "def f(tmp, alvo, dt, s):\n"
        "    tmp.replace(alvo)\n"           # rename de arquivo -> tem de aparecer
        "    os.replace(tmp, alvo)\n"       # dois argumentos -> nao e este padrao
        "    s.replace('a', 'b')\n"         # str.replace -> dois argumentos
        "    return dt.replace(tzinfo=None)\n",  # datetime.replace -> palavra-chave
        encoding="utf-8")
    assert _chamadas_de_rename(velho) == [3]


def test_fila_duravel_sobrevive_ao_destino_ocupado(windows_com_leitor, tmp_path, monkeypatch):
    """`pqueue` e o caso mais caro: perder a gravacao aqui e perder a mensagem que o usuario mandou.

    No codigo velho (`tmp.replace(self.path)`) o PermissionError sobe do `append` e o POST /input
    inteiro vira 500 — a mensagem some sem ninguem saber onde.
    """
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    q = PromptQueue("s")
    q.append("primeira")

    assert [e["text"] for e in PromptQueue("s").load()] == ["primeira"]
    assert windows_com_leitor["chamadas"] == 3      # 2 recusas + a que passou


def test_marcador_de_estado_nao_perde_a_gravacao_calado(windows_com_leitor, tmp_path):
    """`hook_state` e pior que um 500: o `except OSError` de la ENGOLE o erro.

    PermissionError e subclasse de OSError, entao no codigo velho a regravacao falhava, o log saia
    como aviso e o sidecar continuava "awaiting_input" — o proximo boot re-semeava o fantasma que o
    demote existe pra apagar (o mapa em memoria ja estava idle, e so o disco discordava).
    """
    base = tmp_path / "cfg"
    (base / hook_state._SUBDIR).mkdir(parents=True)
    f = base / hook_state._SUBDIR / "sid.json"
    f.write_text(json.dumps({"state": "awaiting_input", "ts": 100.0}), encoding="utf-8")

    hs = hook_state.HookState()
    hs.load_existing([base])
    assert hs.get_state("sid") == ("awaiting_input", 100.0)

    hs.demote_awaiting("sid")

    assert json.loads(f.read_text(encoding="utf-8")) == {"state": "idle", "ts": 100.0}
