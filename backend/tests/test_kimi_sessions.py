import os
from app.adapters.kimi import sessions as ks


def test_workdir_key_e_estavel_e_separa_pastas_diferentes():
    """A PROPRIEDADE, que vale nos dois sistemas — o vetor medido acima so vale no POSIX.

    Sem este caso, o Windows ficaria sem cobertura nenhuma de workdir_key: o unico teste dela
    dependia de um hash medido num caminho que o Windows nem consegue formar.
    """
    a = os.path.abspath(os.path.join(os.sep, "w", "projeto-a"))
    b = os.path.abspath(os.path.join(os.sep, "w", "projeto-b"))
    assert ks.workdir_key(a) == ks.workdir_key(a)          # estavel
    assert ks.workdir_key(a) != ks.workdir_key(b)          # separa
    assert ks.workdir_key(a).startswith("wd_projeto-a_")   # basename no rotulo


def _slug(cwd: str) -> str:
    return ks.workdir_key(cwd).split("_")[1]


def test_workdir_key_slug_segue_o_slugify_do_cli():
    # Regra lida do binario do CLI 0.36.1 (slugifyWorkDirName) e conferida contra as chaves que ele
    # mesmo escreveu em ~/.kimi-code/workspace-trust: minusculas, tudo fora de [a-z0-9._-] vira "-",
    # hifen das pontas cai. Sem o minusculas, pasta com maiuscula no nome ganhava um pre-trust que o
    # CLI nunca achava e a TUI abria no "Trust this folder?".
    assert _slug("/tmp/MinhaPasta") == "minhapasta"
    assert _slug("/tmp/Área de trabalho") == "rea-de-trabalho"
    # Casos de borda: vazio vira "workspace", e corta em 40 chars.
    assert _slug("/@@@") == "workspace"
    assert _slug("/" + "a" * 60) == "a" * 40
    # Corte caindo EM CIMA de um hifen: o CLI faz o strip das pontas duas vezes (antes e depois do
    # slice), entao o "-" que sobra na ponta cai e o slug tem 39 chars, nao 40 terminando em "-".
    assert _slug("/" + "a" * 39 + "-" + "b" * 10) == "a" * 39
