import json
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.config import settings
from app.models import SessionInfo
from app.plano_claude import descobrir


def _evento_tool(identificador: str, caminho: Path, *, sidechain: bool = False) -> str:
    return json.dumps({
        "isSidechain": sidechain,
        "message": {"content": [{"type": "tool_use", "id": identificador, "name": "Write",
                                    "input": {"file_path": str(caminho)}}]},
    })


def _resultado(identificador: str, *, erro: bool = False) -> str:
    return json.dumps({
        "isSidechain": False,
        "message": {"content": [{"type": "tool_result", "tool_use_id": identificador,
                                    "content": "resultado", "is_error": erro}]},
    })


def _assistente(uuid: str, parent: str, *blocos: dict, slug: str | None = None) -> str:
    evento = {
        "type": "assistant", "uuid": uuid, "parentUuid": parent, "isSidechain": False,
        "message": {"content": list(blocos)},
    }
    if slug is not None:
        evento["slug"] = slug
    return json.dumps(evento)


def _resultado_com_parent(uuid: str, parent: str, identificador: str) -> str:
    return json.dumps({
        "type": "user", "uuid": uuid, "parentUuid": parent, "isSidechain": False,
        "message": {"content": [{"type": "tool_result", "tool_use_id": identificador,
                                    "content": "resultado", "is_error": False}]},
    })


def _humano(uuid: str, parent: str, texto: str) -> str:
    return json.dumps({
        "type": "user", "uuid": uuid, "parentUuid": parent, "isSidechain": False,
        "message": {"role": "user", "content": texto},
    })


def _transcript(tmp_path: Path, config: Path, sessao: str, linhas: list[str]) -> Path:
    caminho = config / "projects" / "-projeto" / f"{sessao}.jsonl"
    caminho.parent.mkdir(parents=True, exist_ok=True)
    caminho.write_text("\n".join(linhas), encoding="utf-8")
    return caminho


def test_associa_cada_sessao_ao_plano_que_ela_escreveu(tmp_path):
    config = tmp_path / "conta"
    planos = config / "plans"
    planos.mkdir(parents=True)
    um, dois = planos / "um.md", planos / "dois.md"
    um.write_text("# Um", encoding="utf-8")
    dois.write_text("# Dois", encoding="utf-8")
    t1 = _transcript(tmp_path, config, "s1", [_evento_tool("a", um), _resultado("a")])
    t2 = _transcript(tmp_path, config, "s2", [_evento_tool("b", dois), _resultado("b")])

    assert descobrir(t1, tmp_path).caminho == um
    assert descobrir(t2, tmp_path).caminho == dois


def test_respeita_plans_directory_do_projeto(tmp_path):
    config = tmp_path / "conta"
    projeto = tmp_path / "repo"
    (projeto / ".claude").mkdir(parents=True)
    (projeto / ".claude" / "settings.local.json").write_text(
        json.dumps({"plansDirectory": "documentos/planos"}), encoding="utf-8")
    plano = projeto / "documentos" / "planos" / "custom.md"
    plano.parent.mkdir(parents=True)
    plano.write_text("# Custom", encoding="utf-8")
    transcript = _transcript(tmp_path, config, "s1", [_evento_tool("a", plano), _resultado("a")])

    assert descobrir(transcript, projeto).caminho == plano


def test_ignora_escrita_fora_da_raiz_subagente_e_falha(tmp_path):
    config = tmp_path / "conta"
    planos = config / "plans"
    planos.mkdir(parents=True)
    valido = planos / "valido.md"
    fora = tmp_path / "segredo.md"
    transcript = _transcript(tmp_path, config, "s1", [
        _evento_tool("fora", fora), _resultado("fora"),
        _evento_tool("sub", valido, sidechain=True), _resultado("sub"),
        _evento_tool("falha", valido), _resultado("falha", erro=True),
    ])

    assert descobrir(transcript, tmp_path) is None


def test_mantem_metadado_quando_arquivo_foi_removido(tmp_path):
    config = tmp_path / "conta"
    plano = config / "plans" / "removido.md"
    transcript = _transcript(tmp_path, config, "s1", [_evento_tool("a", plano), _resultado("a")])

    encontrado = descobrir(transcript, tmp_path)
    assert encontrado is not None
    assert encontrado.caminho == plano


def test_slug_nativo_do_transcript_define_o_plano(tmp_path):
    config = tmp_path / "conta"
    transcript = _transcript(tmp_path, config, "s1", [
        json.dumps({"type": "system", "subtype": "compact_boundary",
                    "slug": "plano-nativo", "isSidechain": False}),
        json.dumps({"slug": "plano-nativo", "isSidechain": False, "message": {"content": [
            {"type": "tool_use", "id": "sair", "name": "ExitPlanMode", "input": {"plan": "# Plano"}},
        ]}}),
        _resultado("sair"),
    ])

    encontrado = descobrir(transcript, tmp_path)

    assert encontrado is not None
    assert encontrado.nome == "plano-nativo"
    assert encontrado.caminho == config / "plans" / "plano-nativo.md"


def test_slug_malformado_nao_permite_sair_do_diretorio(tmp_path):
    config = tmp_path / "conta"
    transcript = _transcript(tmp_path, config, "s1", [
        json.dumps({"type": "system", "slug": "../../segredo", "isSidechain": False}),
    ])

    assert descobrir(transcript, tmp_path) is None


@pytest.mark.parametrize("resposta", [
    "User has approved your plan. You can now start coding.",
    "User has approved exiting plan mode. You can now proceed.",
    'User has approved the plan. There is nothing else needed from you now. Please respond with "ok"',
])
def test_aprovacao_nativa_encerra_plano_sem_novo_prompt(tmp_path, resposta):
    config = tmp_path / "conta"
    linhas = [_assistente("proposta", "u1", {
        "type": "tool_use", "id": "exit", "name": "ExitPlanMode", "input": {},
    }, slug="plano-nativo")]
    transcript = _transcript(tmp_path, config, "s1", linhas)
    assert descobrir(transcript, tmp_path).anchor_id == "proposta"

    linhas.append(json.dumps({
        "type": "user", "message": {"content": [{
            "type": "tool_result", "tool_use_id": "exit", "content": resposta,
        }]},
    }))
    linhas.append(_assistente("implementando", "proposta", {"type": "text", "text": "OK"}))
    linhas.extend([
        _evento_tool("progresso", config / "plans" / "plano-nativo.md"),
        _resultado("progresso"),
    ])
    _transcript(tmp_path, config, "s1", linhas)
    assert descobrir(transcript, tmp_path) is None


def test_envio_ao_lider_nao_e_aprovacao(tmp_path):
    config = tmp_path / "conta"
    transcript = _transcript(tmp_path, config, "s1", [
        _assistente("proposta", "u1", {
            "type": "tool_use", "id": "exit", "name": "ExitPlanMode", "input": {},
        }, slug="plano-nativo"),
        json.dumps({"type": "user", "message": {"content": [{
            "type": "tool_result", "tool_use_id": "exit",
            "content": "Your plan has been submitted to the team lead for approval.",
        }]}}),
    ])
    assert descobrir(transcript, tmp_path).anchor_id == "proposta"


def test_slug_de_sessao_normal_sem_plano_nao_anuncia_arquivo(tmp_path):
    config = tmp_path / "conta"
    transcript = _transcript(tmp_path, config, "s1", [
        json.dumps({"type": "user", "slug": "sessao-normal", "isSidechain": False}),
    ])

    assert descobrir(transcript, tmp_path) is None


def test_write_confirmado_prevalece_sobre_slug_sem_arquivo(tmp_path):
    config = tmp_path / "conta"
    plano = config / "plans" / "nome-escolhido.md"
    transcript = _transcript(tmp_path, config, "s1", [
        json.dumps({"type": "user", "slug": "slug-generico", "isSidechain": False}),
        _evento_tool("escrita", plano),
        _resultado("escrita"),
    ])

    encontrado = descobrir(transcript, tmp_path)
    assert encontrado is not None
    assert encontrado.caminho == plano


def test_resposta_humana_encerra_proposta_anterior(tmp_path):
    config = tmp_path / "conta"
    plano = config / "plans" / "turno.md"
    transcript = _transcript(tmp_path, config, "s1", [
        _humano("u1", "root", "crie um plano"),
        _assistente("a-write", "u1", {
            "type": "tool_use", "id": "write-1", "name": "Write",
            "input": {"file_path": str(plano)},
        }),
        _resultado_com_parent("r1", "a-write", "write-1"),
        _assistente("a-plan", "r1", {"type": "text", "text": "Plano confirmado."}),
        _humano("u2", "a-plan", "pergunta posterior"),
        _assistente("a-longe", "u2", {"type": "text", "text": "Resposta posterior alheia."}),
    ])

    encontrado = descobrir(transcript, tmp_path)

    assert encontrado is None


def test_escrita_sem_texto_nao_reaparece_depois_de_outro_prompt(tmp_path):
    config = tmp_path / "conta"
    plano = config / "plans" / "sem-texto.md"
    transcript = _transcript(tmp_path, config, "s1", [
        _humano("u1", "root", "crie o plano"),
        _assistente("a-write", "u1", {
            "type": "tool_use", "id": "write-1", "name": "Write",
            "input": {"file_path": str(plano)},
        }),
        _resultado_com_parent("r1", "a-write", "write-1"),
        _humano("u2", "a-write", "outra pergunta"),
    ])

    encontrado = descobrir(transcript, tmp_path)

    assert encontrado is None


def test_plano_novo_no_turno_seguinte_troca_caminho_e_ancora(tmp_path):
    config = tmp_path / "conta"
    primeiro = config / "plans" / "primeiro.md"
    segundo = config / "plans" / "segundo.md"
    transcript = _transcript(tmp_path, config, "s1", [
        _humano("u1", "root", "primeiro"),
        _assistente("a-write-1", "u1", {
            "type": "tool_use", "id": "write-1", "name": "Write",
            "input": {"file_path": str(primeiro)},
        }),
        _resultado_com_parent("r1", "a-write-1", "write-1"),
        _assistente("a-plan-1", "r1", {"type": "text", "text": "Plano primeiro."}),
        _humano("u2", "a-plan-1", "segundo"),
        _assistente("a-write-2", "u2", {
            "type": "tool_use", "id": "write-2", "name": "Write",
            "input": {"file_path": str(segundo)},
        }),
        _resultado_com_parent("r2", "a-write-2", "write-2"),
        _assistente("a-plan-2", "r2", {"type": "text", "text": "Plano segundo."}),
    ])

    encontrado = descobrir(transcript, tmp_path)

    assert encontrado is not None
    assert encontrado.caminho == segundo
    assert encontrado.anchor_id == "a-plan-2"


@pytest.fixture
def cliente(monkeypatch):
    settings.auth_token = "segredo"
    import app.api as api_mod

    async def info(_nome: str):
        return api_mod._info_teste

    monkeypatch.setattr(api_mod, "_cached_info", info)
    return TestClient(api_mod.app), api_mod


def test_endpoint_leve_nao_exige_arquivo_existente(tmp_path, cliente):
    client, api_mod = cliente
    config = tmp_path / "conta"
    plano = config / "plans" / "removido.md"
    transcript = _transcript(tmp_path, config, "s1", [_evento_tool("a", plano), _resultado("a")])
    api_mod._info_teste = SessionInfo(name="sessao", cwd=str(tmp_path), jsonl=str(transcript))

    resposta = client.get(
        "/api/sessions/sessao/plan-preview?content=false",
        headers={"Authorization": "Bearer segredo"},
    )

    assert resposta.status_code == 200
    assert resposta.json() == {"name": "removido", "path": str(plano)}


def test_endpoint_leve_expoe_ancora_da_resposta_do_turno(tmp_path, cliente):
    client, api_mod = cliente
    config = tmp_path / "conta"
    plano = config / "plans" / "plano.md"
    transcript = _transcript(tmp_path, config, "s1", [
        _humano("u1", "root", "crie o plano"),
        _assistente("a-write", "u1", {
            "type": "tool_use", "id": "write-1", "name": "Write",
            "input": {"file_path": str(plano)},
        }),
        _resultado_com_parent("r1", "a-write", "write-1"),
        _assistente("a-plan", "r1", {"type": "text", "text": "Plano confirmado."}),
    ])
    api_mod._info_teste = SessionInfo(name="sessao", cwd=str(tmp_path), jsonl=str(transcript))

    resposta = client.get(
        "/api/sessions/sessao/plan-preview?content=false",
        headers={"Authorization": "Bearer segredo"},
    )

    assert resposta.status_code == 200
    assert resposta.json()["anchor_id"] == "a-plan"


def test_write_confirmado_prevalece_sobre_exit_plan_mode_e_endpoint_preserva_conteudo(tmp_path, cliente):
    client, api_mod = cliente
    config = tmp_path / "conta"
    plano = config / "plans" / "chosen.md"
    plano.parent.mkdir(parents=True)
    plano.write_text("# Escolhido", encoding="utf-8")
    transcript = _transcript(tmp_path, config, "s1", [
        _humano("u1", "root", "crie o plano"),
        _assistente("a-write", "u1", {
            "type": "tool_use", "id": "write-1", "name": "Write",
            "input": {"file_path": str(plano)},
        }),
        _resultado_com_parent("r1", "a-write", "write-1"),
        _assistente("a-exit", "r1", {
            "type": "tool_use", "id": "exit-1", "name": "ExitPlanMode", "input": {"plan": "# genérico"},
        }, slug="slug-generico"),
        _resultado_com_parent("r2", "a-exit", "exit-1"),
        _assistente("a-plan", "r2", {"type": "text", "text": "Plano escolhido."}),
    ])
    api_mod._info_teste = SessionInfo(name="sessao", cwd=str(tmp_path), jsonl=str(transcript))

    resposta = client.get(
        "/api/sessions/sessao/plan-preview",
        headers={"Authorization": "Bearer segredo"},
    )

    assert resposta.status_code == 200
    assert resposta.json() == {
        "name": "chosen", "path": str(plano), "anchor_id": "a-plan", "markdown": "# Escolhido",
    }


def test_endpoint_conteudo_retorna_404_se_plano_foi_removido(tmp_path, cliente):
    client, api_mod = cliente
    config = tmp_path / "conta"
    plano = config / "plans" / "removido.md"
    transcript = _transcript(tmp_path, config, "s1", [_evento_tool("a", plano), _resultado("a")])
    api_mod._info_teste = SessionInfo(name="sessao", cwd=str(tmp_path), jsonl=str(transcript))

    resposta = client.get(
        "/api/sessions/sessao/plan-preview",
        headers={"Authorization": "Bearer segredo"},
    )

    assert resposta.status_code == 404
    assert resposta.json()["detail"]["code"] == "erro_plano_removido"
