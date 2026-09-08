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
    assert novo["hooks"]["PreToolUse"] == [pessoal, _grupo("igual")]
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
