"""Os arquivos de `docs/atualizacoes/` DESTE repo são válidos.

Existe porque um passo malformado é descartado em silêncio na máquina de quem usa: ele some de
`pendentes()` com um aviso no log de um processo destacado que ninguém lê, e o efeito que ele
deveria ter simplesmente nunca acontece — em nenhuma máquina, sem que apareça em lugar nenhum.

O lugar certo de pegar isso é aqui, antes de sair da máquina de quem publica.
"""
from app import atualizacoes


def _arquivos():
    pasta = atualizacoes._dir()
    if not pasta.is_dir():
        return []
    return [a for a in sorted(pasta.glob("*.md")) if not a.name.upper().startswith("README")]


def test_todo_passo_declarado_e_valido():
    ruins = []
    for arquivo in _arquivos():
        if atualizacoes._passo(arquivo) is None:
            ruins.append(arquivo.name)
    assert not ruins, (
        f"passos que o app vai IGNORAR calado: {ruins}. "
        "Todo passo precisa de 'titulo', e de 'prova' sempre que tiver 'comando'."
    )


def test_ids_nao_se_repetem():
    """Id repetido faz o segundo passo nunca rodar: o registro já o considera aplicado."""
    vistos: dict[str, str] = {}
    repetidos = []
    for arquivo in _arquivos():
        p = atualizacoes._passo(arquivo)
        if p and p["id"] in vistos:
            repetidos.append(f"{p['id']} ({vistos[p['id']]} e {arquivo.name})")
        elif p:
            vistos[p["id"]] = arquivo.name
    assert not repetidos, f"ids repetidos: {repetidos}"
