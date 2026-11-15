"""Os call sites do `tmp+rename` passam pelo `atomico.substituir` — inclusive os que escrevem
`tmp.replace(alvo)`, o metodo do `Path`.

O commit anterior converteu os 14 lugares escritos como `os.replace(tmp, alvo)` e deixou de fora 19
escritos como `tmp.replace(alvo)`. Nao e outra classe de bug: `Path.replace` E `os.replace` por
baixo (`pathlib` chama `os.replace(self, target)`), entao no Windows ele levanta o mesmo
PermissionError [WinError 5] quando alguem esta com o destino aberto so pra leitura — e ficaram de
fora justamente os sidecars que tem leitor concorrente por desenho (fila duravel, marcador lido por
hook em outro processo, cache de tarifas).
"""
import ast
from pathlib import Path

from app import atomico

_APP = Path(atomico.__file__).parent


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
