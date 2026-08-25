"""`tmux.join_cmd` — a citacao do comando no dialeto que o multiplexador entende.

O comando que sobe uma sessao e uma STRING unica, e o multiplexador a reparte. O `shlex.join` cita
no dialeto POSIX, e o `sh` remonta um apostrofo embutido porque entende o idioma `'a'"'"'b'`. O
psmux NAO entende esse idioma — medido nesta VM em 22/08/2026, lendo o argv do outro lado:

    shlex.join(["python", d, "com'apostrofo"])  -> chegou ['com', "'", 'apostrofo']

e medido o que FUNCIONA la:

    aspas simples + apostrofo DOBRADO  -> apostrofo, espaco, `$` e `C:\\Users\\x\\proj` inteiros
    aspas DUPLAS                       -> apostrofo e espaco passam, mas `$cifrao` e EXPANDIDO
    `\\'`                              -> o comando nem chega a executar

Por isso aspas simples, e nao duplas: proteger o `$` e propriedade que o shlex.join ja dava e que
nao se pode perder na troca.

Exposicao hoje e quase nula (os argumentos sao uuid e ids validados por regex, e o `cwd` NAO entra
no comando — vai no `-c` do new-session), mas o modo de falha e o pior tipo: parte calado, e o
que sobra ainda parece um comando.
"""
import shlex

import pytest

from app import tmux


@pytest.fixture
def win(monkeypatch):
    monkeypatch.setattr(tmux.os, "name", "nt")


@pytest.fixture
def posix(monkeypatch):
    monkeypatch.setattr(tmux.os, "name", "posix")


def test_posix_e_o_shlex_join_byte_por_byte(posix):
    """O Linux nao pode mudar: la o idioma POSIX funciona e trocar so arriscaria."""
    for args in (["claude", "--resume", "abc-123"],
                 ["pi", "--session-id", "x", "--model", "openrouter/~anthropic/opus"],
                 ["hangar-engine", "--exec", "kimi", "--", "claude", "--session-id", "u"],
                 ["x", "com espaco", "com'apostrofo", "com$cifrao"]):
        assert tmux.join_cmd(args) == shlex.join(args)


def test_windows_dobra_o_apostrofo_em_vez_do_idioma_posix(win):
    assert tmux.join_cmd(["p", "com'apostrofo"]) == "p 'com''apostrofo'"
    # O idioma que o psmux nao entende NAO pode aparecer.
    assert '"\'"' not in tmux.join_cmd(["p", "com'apostrofo"])


def test_windows_usa_aspas_SIMPLES_para_proteger_o_cifrao(win):
    """Aspas duplas fariam o `$cifrao` ser expandido — medido, chega como 'com'."""
    saida = tmux.join_cmd(["p", "com$cifrao"])
    assert saida == "p 'com$cifrao'"
    assert '"' not in saida


def test_windows_preserva_espaco_e_caminho_do_windows(win):
    assert tmux.join_cmd(["p", "com espaco"]) == "p 'com espaco'"
    assert tmux.join_cmd(["p", r"C:\Users\x\proj"]) == r"p 'C:\Users\x\proj'"


def test_windows_nao_cita_o_que_nao_precisa(win):
    """Mesma politica do shlex: argumento simples entra cru, senao todo comando viraria ruido."""
    assert tmux.join_cmd(["claude", "--resume", "abc-123_x.y"]) == "claude --resume abc-123_x.y"
    assert tmux.join_cmd(["a", ""]) == "a ''"


def test_registry_nao_usa_mais_shlex_join():
    """Uma citacao so. Duas divergem no proximo conserto — foi o padrao do `/bin/sh` do stop."""
    import pathlib
    fonte = pathlib.Path(tmux.__file__).parent.joinpath("registry.py").read_text(encoding="utf-8")
    assert "shlex.join(" not in fonte
    assert "tmux.join_cmd(" in fonte
