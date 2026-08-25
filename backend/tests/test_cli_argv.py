"""Nenhum `subprocess` do backend chama CLI pelo NOME CRU — o argv leva caminho resolvido.

Por que isso e uma trava e nao um detalhe: no Windows o `CreateProcess` procura no PATH mas so
completa `.exe`. CLI instalada por npm vira um `.CMD` (medido nesta VM: `pi` ->
`…\\npm\\pi.CMD`), e `subprocess.run(["pi", …])` levanta `FileNotFoundError [WinError 2]` com o
`pi` funcionando perfeitamente no terminal de quem digita. O sintoma nao parece com a causa: a tela
de abertura respondia 502 "pi --list-models falhou" enquanto o `cli_probe`, que resolve por
`shutil.which`, jurava que o pi estava la.

`shutil.which` aplica o PATHEXT (e no POSIX e o mesmo `PATH` de sempre), entao a regra vale nos dois
sistemas e nao tem ramo condicional.
"""
import ast
from pathlib import Path

import pytest

from app import pi_catalog

_APP = Path(pi_catalog.__file__).parent

# A regra NAO e "nome cru e sempre defeito" — e "nome cru so vale pra quem e binario de verdade".
# O que quebra e CLI instalada por gerenciador de pacote de linguagem, que no Windows vira um script
# `.CMD`/`.BAT` com o nome do comando. Medido nesta VM em 22/08/2026, com `shutil.which`:
#
#     pi        -> …\\AppData\\Roaming\\npm\\pi.CMD      <- npm, e o unico .CMD -> era o defeito
#     tmux      -> …\\WinGet\\Links\\tmux.EXE
#     git       -> C:\\Program Files\\Git\\mingw64\\bin\\git.EXE
#     claude    -> …\\.local\\bin\\claude.EXE
#     tailscale -> C:\\Program Files\\Tailscale\\tailscale.EXE
#     systemctl -> None (nao existe no Windows; o caminho que o chama e POSIX-only)
#
# Os cinco de baixo ficam liberados COM essa medida, nao por concessao — e cada um deles falha alto
# se sumir (sem tmux o app nem sobe; sem git a aba de repo morre inteira), diferente do `pi`, que
# derrubava UMA tela com um erro que culpava o comando errado. O `tmux._run` ainda tem o agravante
# de ser o caminho mais quente do backend (roda por poll, por sessao): um `which` ali se paga em
# nada. A trava existe pro PROXIMO nome — `kimi`, `codex`, `npm` e qualquer coisa vinda de npm
# entram como `.CMD` e precisam ser resolvidas.
_LIBERADOS = {"tmux", "git", "claude", "tailscale", "systemctl"}


def _nome_cru_em(arquivo: Path) -> list[tuple[int, str]]:
    """(linha, nome) de cada `subprocess.run([...])`/`Popen([...])` cujo argv[0] e literal de texto.

    So o argv[0]: os argumentos seguintes sao do comando, nao do resolvedor. Lista montada em
    variavel nao entra — quem monta ja resolveu (e o teste nao adivinha o conteudo).
    """
    achados: list[tuple[int, str]] = []
    for no in ast.walk(ast.parse(arquivo.read_text(encoding="utf-8"))):
        if not isinstance(no, ast.Call) or not isinstance(no.func, ast.Attribute):
            continue
        if no.func.attr not in ("run", "Popen") or not no.args:
            continue
        argv = no.args[0]
        if not isinstance(argv, ast.List) or not argv.elts:
            continue
        primeiro = argv.elts[0]
        if isinstance(primeiro, ast.Constant) and isinstance(primeiro.value, str):
            nome = primeiro.value
            if nome not in _LIBERADOS:
                achados.append((no.lineno, nome))
    return achados


def test_nenhum_subprocess_chama_cli_pelo_nome_cru():
    achados = {str(f.relative_to(_APP)): linhas
               for f in sorted(_APP.rglob("*.py"))
               if (linhas := _nome_cru_em(f))}
    assert achados == {}, ("resolva com shutil.which antes de montar o argv (ver o docstring "
                           f"deste arquivo): {achados}")


@pytest.mark.parametrize("codigo, esperado", [
    ('subprocess.run(["pi", "--list-models"])', [(1, "pi")]),
    ('subprocess.Popen(["kimi", "--json"])', [(1, "kimi")]),
    ('subprocess.run(["tmux", "list-panes"])', []),            # liberado, e medido
    ('subprocess.run([caminho, "--version"])', []),            # ja resolvido
    ('subprocess.run([shutil.which("pi"), "-x"])', []),        # resolvido na hora
    ('subprocess.run(argv)', []),                              # lista montada fora
    ('r.run(["pi"])', [(1, "pi")]),                            # o modulo pode ter outro nome
])
def test_a_varredura_reconhece_o_que_deve(tmp_path, codigo, esperado):
    """Contra-prova: sem ela, um erro no reconhecimento faria o teste de cima passar vazio."""
    f = tmp_path / "amostra.py"
    f.write_text(codigo + "\n", encoding="utf-8")
    assert _nome_cru_em(f) == esperado
