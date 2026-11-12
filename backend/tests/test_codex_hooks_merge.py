"""Mesclagem identifica hooks pelo matcher e recusa leituras parciais ou inválidas."""
from copy import deepcopy

import pytest

from app.codex_arquivos import mesclar_hooks


def _grupo(command, matcher="Bash", **extras):
    return {"matcher": matcher, "hooks": [{"type": "command", "command": command}], **extras}


def test_mesmo_comando_em_matcher_pessoal_nao_e_removido():
    fonte = {"hooks": {"PreToolUse": [_grupo("igual")]}}
    pessoal = _grupo("igual", "Read", descricao="meu")
    atual = {"hooks": {"PreToolUse": [_grupo("igual"), pessoal]}}
    novo = mesclar_hooks(atual, fonte, {})
    assert novo["hooks"]["PreToolUse"] == [_grupo("igual"), pessoal]
    assert mesclar_hooks(novo, fonte, fonte) == novo


def test_troca_matcher_retira_identidade_antiga_preserva_demais():
    anterior = {"hooks": {"PreToolUse": [_grupo("comando", "Bash")]}}
    fonte = {"hooks": {"PreToolUse": [_grupo("comando", "Edit")]}}
    pessoal = _grupo("comando", "Read")
    atual = {"hooks": {"PreToolUse": [_grupo("comando", "Bash"), pessoal],
                       "PostToolUse": [_grupo("comando", "Bash")]}}
    novo = mesclar_hooks(atual, fonte, anterior)
    assert novo["hooks"]["PreToolUse"] == [pessoal, _grupo("comando", "Edit")]
    assert novo["hooks"]["PostToolUse"] == atual["hooks"]["PostToolUse"]


def test_grupo_misto_mantem_metadata_e_hook_pessoal():
    grupo = _grupo("gerenciado", descricao="contexto do grupo", outro={"valor": 1})
    grupo["hooks"].append({"type": "command", "command": "pessoal", "timeout": 42})
    atual = {"description": "arquivo pessoal", "hooks": {"PreToolUse": [grupo]}}
    anterior = {"hooks": {"PreToolUse": [_grupo("gerenciado")]}}
    novo = mesclar_hooks(atual, {"hooks": {}}, anterior)
    assert novo == {"description": "arquivo pessoal", "hooks": {"PreToolUse": [{
        **grupo, "hooks": [{"type": "command", "command": "pessoal", "timeout": 42}],
    }]}}
    assert len(atual["hooks"]["PreToolUse"][0]["hooks"]) == 2


def test_fonte_vazia_remove_so_identidades_do_manifesto():
    anterior = {"hooks": {"PreToolUse": [_grupo("velho")]}}
    atual = {"hooks": {"PreToolUse": [_grupo("velho"), _grupo("velho", "Read")],
                       "Stop": [_grupo("fim", "")]}}
    novo = mesclar_hooks(atual, {}, anterior)
    assert novo == {"hooks": {"PreToolUse": [_grupo("velho", "Read")], "Stop": [_grupo("fim", "")]}}


def test_hook_nao_command_e_preservado_e_nao_duplica():
    prompt = {"matcher": "Bash", "hooks": [{"type": "prompt", "prompt": "importado"}]}
    pessoal = {"matcher": "Bash", "hooks": [{"type": "prompt", "prompt": "pessoal"}]}
    fonte = {"hooks": {"PreToolUse": [prompt]}}
    atual = {"hooks": {"PreToolUse": [pessoal, prompt]}}
    assert mesclar_hooks(atual, fonte, fonte) == atual


def test_adicionar_hook_preserva_grupos_existentes_e_indices():
    anterior = {"hooks": {"PreToolUse": [_grupo("review"), _grupo("rtk")]}}
    atual = deepcopy(anterior)
    atual["hooks"]["PreToolUse"].append(_grupo("guarda"))
    fonte = deepcopy(anterior)
    fonte["hooks"]["PreToolUse"].insert(0, _grupo("novo"))

    novo = mesclar_hooks(atual, fonte, anterior)

    assert novo["hooks"]["PreToolUse"] == [*atual["hooks"]["PreToolUse"], _grupo("novo")]
    assert mesclar_hooks(novo, fonte, fonte) == novo


def test_atualizacao_de_hook_em_grupo_misto_preserva_slots_e_metadata():
    grupo = _grupo("review", descricao="pessoal")
    grupo["hooks"].insert(0, {"type": "command", "command": "pessoal"})
    atual = {"hooks": {"PreToolUse": [grupo, _grupo("guarda")]}}
    anterior = {"hooks": {"PreToolUse": [_grupo("review")]}}
    fonte = deepcopy(anterior)
    fonte["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"] = 12

    novo = mesclar_hooks(atual, fonte, anterior)

    esperado = deepcopy(atual)
    esperado["hooks"]["PreToolUse"][0]["hooks"][1]["timeout"] = 12
    assert novo == esperado
    assert mesclar_hooks(novo, fonte, fonte) == novo


def test_atualizacao_de_metadata_do_grupo_gerenciado_nao_move_outros():
    anterior = {"hooks": {"PreToolUse": [_grupo("review", descricao="antigo")]}}
    atual = deepcopy(anterior)
    atual["hooks"]["PreToolUse"].append(_grupo("guarda"))
    fonte = {"hooks": {"PreToolUse": [_grupo("review", description="novo")]}}

    novo = mesclar_hooks(atual, fonte, anterior)

    assert novo["hooks"]["PreToolUse"] == [_grupo("review", description="novo"), _grupo("guarda")]


def test_remocao_no_grupo_atualiza_metadata_na_primeira_reconciliacao():
    grupo = _grupo("review", description="antigo")
    grupo["hooks"].append({"type": "command", "command": "rtk"})
    atual = {"hooks": {"PreToolUse": [grupo]}}
    fonte = {"hooks": {"PreToolUse": [_grupo("review", description="novo")]}}

    novo = mesclar_hooks(atual, fonte, atual)

    assert novo == fonte
    assert mesclar_hooks(novo, fonte, fonte) == novo


def test_adicao_no_grupo_atualiza_metadata_sem_mover_hooks_existentes():
    anterior = {"hooks": {"PreToolUse": [_grupo("review", description="antigo")]}}
    atual = deepcopy(anterior)
    atual["hooks"]["PreToolUse"].append(_grupo("guarda"))
    grupo = _grupo("review", description="novo")
    grupo["hooks"].append({"type": "command", "command": "rtk"})
    fonte = {"hooks": {"PreToolUse": [grupo]}}

    novo = mesclar_hooks(atual, fonte, anterior)

    assert novo["hooks"]["PreToolUse"] == [
        _grupo("review", description="novo"), _grupo("guarda"),
        _grupo("rtk", description="novo"),
    ]
    assert mesclar_hooks(novo, fonte, fonte) == novo


@pytest.mark.parametrize("vazio_primeiro", [False, True])
def test_grupo_consumido_nao_se_confunde_com_grupo_originalmente_vazio(vazio_primeiro):
    grupo = _grupo("review")
    vazio = {"matcher": "Bash", "hooks": []}
    grupos = [vazio, grupo] if vazio_primeiro else [grupo, vazio]
    fonte = {"hooks": {"PreToolUse": grupos}}

    novo = mesclar_hooks(fonte, fonte, fonte)

    assert novo == fonte
    assert mesclar_hooks(novo, fonte, fonte) == novo


@pytest.mark.parametrize("invalido", [
    None, [], {"hooks": None}, {"hooks": []}, {"hooks": {"Stop": {}}},
    {"hooks": {"Stop": [None]}}, {"hooks": {"Stop": [{}]}},
    {"hooks": {"Stop": [{"hooks": None}]}},
    {"hooks": {"Stop": [{"matcher": [], "hooks": []}]}},
    {"hooks": {"Stop": [{"hooks": [None]}]}},
    {"hooks": {"Stop": [{"hooks": [{}]}]}},
    {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": []}]}]}},
    {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": " "}]}]}},
    {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "fim", "timeout": "3"}]}]}},
    {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "fim", "timeout": True}]}]}},
    {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "fim", "timeout": float("nan")}]}]}},
])
@pytest.mark.parametrize("posicao", [0, 1, 2])
def test_valida_os_tres_documentos_antes_de_qualquer_mesclagem(invalido, posicao):
    valido = {"hooks": {"PreToolUse": [_grupo("gerenciado")]}}
    documentos = [deepcopy(valido), deepcopy(valido), deepcopy(valido)]
    documentos[posicao] = invalido
    with pytest.raises(ValueError, match="inválido"):
        mesclar_hooks(*documentos)
    for indice, documento in enumerate(documentos):
        if indice != posicao:
            assert documento == valido


def test_grupos_vazios_sao_idempotentes_e_preservam_metadados_pessoais():
    pessoal = {"matcher": "Read", "hooks": [], "description": "meu"}
    gerenciado = {"hooks": []}
    fonte = {"hooks": {"Stop": [gerenciado]}}
    atual = {"hooks": {"Stop": [pessoal, gerenciado]}}
    assert mesclar_hooks(atual, fonte, fonte) == atual
