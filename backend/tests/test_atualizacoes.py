"""Passos por versão: leitura, registro do que já rodou, e a prova que decide se rodou de verdade."""
import json

import pytest

from app import atualizacoes


@pytest.fixture
def passos(tmp_path, monkeypatch):
    """Pasta de passos e sidecar de aplicados, os dois isolados em tmp."""
    monkeypatch.setattr(atualizacoes, "REPO", tmp_path)
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "cfg"))
    d = tmp_path / "docs" / "atualizacoes"
    d.mkdir(parents=True)
    return d


def _escreve(pasta, nome, **campos):
    corpo = campos.pop("texto", "o que mudou")
    fm = "\n".join(f"{k}: {v}" for k, v in campos.items())
    (pasta / f"{nome}.md").write_text(f"---\n{fm}\n---\n\n{corpo}\n", encoding="utf-8")


# ─── Leitura ───────────────────────────────────────────────────────────────────────────────────

def test_le_frontmatter_e_corpo(passos):
    _escreve(passos, "2026-01-01-um", id="2026-01-01-um", titulo="Primeiro",
             comando="true", prova="true", destrutivo="false", texto="Texto pra pessoa ler.")
    (p,) = atualizacoes.todos()
    assert p["id"] == "2026-01-01-um" and p["titulo"] == "Primeiro"
    assert p["comando"] == "true" and p["destrutivo"] is False
    assert p["texto"] == "Texto pra pessoa ler."


def test_valor_com_dois_pontos_sobrevive(passos):
    _escreve(passos, "x", id="x", titulo="Um: com dois pontos", comando="echo a:b")
    (p,) = atualizacoes.todos()
    assert p["titulo"] == "Um: com dois pontos" and p["comando"] == "echo a:b"


def test_sem_titulo_e_ignorado_sem_derrubar_o_resto(passos):
    """Arquivo malformado não pode travar a atualização de todo mundo."""
    _escreve(passos, "quebrado", id="quebrado", comando="true")
    _escreve(passos, "bom", id="bom", titulo="Vale")
    assert [p["id"] for p in atualizacoes.todos()] == ["bom"]


def test_readme_nao_e_passo(passos):
    (passos / "README.md").write_text("# instruções\n", encoding="utf-8")
    _escreve(passos, "um", id="um", titulo="Vale")
    assert [p["id"] for p in atualizacoes.todos()] == ["um"]


def test_ordem_e_por_id(passos):
    _escreve(passos, "2026-03-03-c", id="2026-03-03-c", titulo="C")
    _escreve(passos, "2026-01-01-a", id="2026-01-01-a", titulo="A")
    assert [p["titulo"] for p in atualizacoes.todos()] == ["A", "C"]


# ─── Registro ──────────────────────────────────────────────────────────────────────────────────

@pytest.mark.parametrize("conteudo", ["null", '{"a": 1}', "texto solto"])
def test_aplicados_exige_lista(passos, conteudo):
    alvo = atualizacoes._caminho_aplicados()
    alvo.parent.mkdir(parents=True, exist_ok=True)
    alvo.write_text(conteudo, encoding="utf-8")
    assert atualizacoes.aplicados() == set()


def test_passo_ja_aplicado_nao_fica_pendente(passos):
    _escreve(passos, "um", id="um", titulo="Um")
    _escreve(passos, "dois", id="dois", titulo="Dois")
    atualizacoes.marcar("um")
    assert [p["id"] for p in atualizacoes.pendentes()] == ["dois"]


def test_marcar_todos_nao_roda_nada(passos, tmp_path):
    """Instalação do zero: tudo já foi feito pelo instalador, nada pode rodar de novo."""
    marca = tmp_path / "rodou"
    _escreve(passos, "um", id="um", titulo="Um", comando=f"touch {marca}")
    assert atualizacoes.marcar_todos() == 1
    assert not marca.exists()
    assert atualizacoes.pendentes() == []


def test_destrutivo_pode_ficar_de_fora(passos):
    _escreve(passos, "um", id="um", titulo="Um", destrutivo="false")
    _escreve(passos, "dois", id="dois", titulo="Dois", destrutivo="true")
    assert [p["id"] for p in atualizacoes.pendentes(incluir_destrutivos=False)] == ["um"]
    assert len(atualizacoes.pendentes()) == 2


# ─── Aplicar ───────────────────────────────────────────────────────────────────────────────────

def test_aplica_e_marca(passos, tmp_path):
    marca = tmp_path / "feito"
    _escreve(passos, "um", id="um", titulo="Um", comando=f"touch {marca}", prova=f"test -f {marca}")
    assert atualizacoes.aplicar_pendentes() == ["um"]
    assert marca.exists()
    assert atualizacoes.aplicados() == {"um"}


def test_comando_que_falha_nao_marca(passos):
    _escreve(passos, "um", id="um", titulo="Um", comando="exit 3")
    with pytest.raises(atualizacoes.PassoFalhou):
        atualizacoes.aplicar_pendentes()
    assert atualizacoes.aplicados() == set()


def test_prova_que_falha_nao_marca(passos):
    """Comando com exit 0 e efeito ausente é justamente o que a prova existe pra pegar."""
    _escreve(passos, "um", id="um", titulo="Um", comando="true", prova="false")
    with pytest.raises(atualizacoes.PassoFalhou) as e:
        atualizacoes.aplicar_pendentes()
    assert "verificacao" in str(e.value)
    assert atualizacoes.aplicados() == set()


def test_para_no_primeiro_erro(passos, tmp_path):
    """Passo costuma depender do anterior; seguir em frente deixaria estado que ninguém desenhou."""
    depois = tmp_path / "nao-deveria"
    _escreve(passos, "1-quebra", id="1-quebra", titulo="Quebra", comando="exit 1")
    _escreve(passos, "2-depois", id="2-depois", titulo="Depois", comando=f"touch {depois}")
    with pytest.raises(atualizacoes.PassoFalhou):
        atualizacoes.aplicar_pendentes()
    assert not depois.exists()


def test_rodar_duas_vezes_nao_repete(passos, tmp_path):
    contador = tmp_path / "n"
    _escreve(passos, "um", id="um", titulo="Um",
             comando=f"echo x >> {contador}", prova=f"test -f {contador}")
    atualizacoes.aplicar_pendentes()
    atualizacoes.aplicar_pendentes()
    assert contador.read_text().count("x") == 1


def test_sidecar_e_json_valido(passos):
    _escreve(passos, "um", id="um", titulo="Um")
    atualizacoes.marcar("um")
    assert json.loads(atualizacoes._caminho_aplicados().read_text(encoding="utf-8")) == ["um"]
