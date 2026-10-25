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
             comando="echo oi", prova="docs", destrutivo="false", texto="Texto pra pessoa ler.")
    (p,) = atualizacoes.todos()
    assert p["id"] == "2026-01-01-um" and p["titulo"] == "Primeiro"
    assert p["comando"] == "echo oi" and p["prova"] == ["docs"] and p["destrutivo"] is False
    assert p["texto"] == "Texto pra pessoa ler."


def test_valor_com_dois_pontos_sobrevive(passos):
    _escreve(passos, "x", id="x", titulo="Um: com dois pontos", comando="echo a:b", prova="docs")
    (p,) = atualizacoes.todos()
    assert p["titulo"] == "Um: com dois pontos" and p["comando"] == "echo a:b"


def test_comando_especifico_da_plataforma_vence_o_generico(passos, monkeypatch):
    _escreve(passos, "x", id="x", titulo="Instala", comando="echo generico",
             comando_posix="echo posix", comando_windows="echo windows", prova="docs")
    monkeypatch.setattr(atualizacoes, "_WINDOWS", False, raising=False)
    assert atualizacoes.todos()[0]["comando"] == "echo posix"
    monkeypatch.setattr(atualizacoes, "_WINDOWS", True)
    assert atualizacoes.todos()[0]["comando"] == "echo windows"


def test_passo_so_posix_no_windows_vira_texto_e_nao_cobra_prova(passos, monkeypatch):
    """A prova é do efeito do comando posix; cobrá-la no Windows travava a atualização lá."""
    _escreve(passos, "x", id="x", titulo="Só Linux", comando_posix="touch marca", prova="marca")
    monkeypatch.setattr(atualizacoes, "_WINDOWS", True)
    (p,) = atualizacoes.todos()
    assert p["comando"] == "" and p["prova"] == []
    atualizacoes.aplicar(p)
    assert "x" in atualizacoes.aplicados()


def test_passo_sem_comando_nenhum_ainda_cobra_prova(passos):
    """Passo só de texto pode provar que o código chegou; isso não some."""
    _escreve(passos, "x", id="x", titulo="Chegou", prova="nao-existe")
    (p,) = atualizacoes.todos()
    assert p["prova"] == ["nao-existe"]
    with pytest.raises(atualizacoes.PassoFalhou):
        atualizacoes.aplicar(p)


def test_sem_titulo_e_ignorado_sem_derrubar_o_resto(passos):
    """Arquivo malformado não pode travar a atualização de todo mundo."""
    _escreve(passos, "quebrado", id="quebrado", comando="echo oi", prova="docs")
    _escreve(passos, "bom", id="bom", titulo="Vale")
    assert [p["id"] for p in atualizacoes.todos()] == ["bom"]


def test_comando_sem_prova_e_recusado(passos):
    """Comando sem prova é o defeito que a feature existe pra eliminar: exit 0 vira "deu certo"."""
    _escreve(passos, "sem-prova", id="sem-prova", titulo="Faz algo", comando="echo oi")
    assert atualizacoes.todos() == []


def test_passo_so_de_texto_dispensa_prova(passos):
    """Sem comando não há efeito a verificar — é só changelog."""
    _escreve(passos, "so-texto", id="so-texto", titulo="Aviso", texto="Leia isso.")
    assert [p["id"] for p in atualizacoes.todos()] == ["so-texto"]


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
    _escreve(passos, "um", id="um", titulo="Um", comando=f"touch {marca}", prova=str(marca))
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
    _escreve(passos, "um", id="um", titulo="Um", comando=f"touch {marca}", prova=str(marca))
    assert atualizacoes.aplicar_pendentes() == ["um"]
    assert marca.exists()
    assert atualizacoes.aplicados() == {"um"}


def test_falha_mostra_o_motivo_marcado_e_nao_a_cauda(passos):
    """O instalador imprime o motivo bem antes do portão do fim, que ocupa as últimas linhas."""
    script = passos.parent.parent / "falha.sh"
    script.write_text("echo '##HANGAR-FALHA## tarefas agendadas: Porta 8765 ocupada por outro processo'\n"
                      "for i in 1 2 3 4 5 6 7 8 9 10; do echo \"portao do fim $i\"; done\nexit 1\n",
                      encoding="utf-8")
    _escreve(passos, "um", id="um", titulo="Um", comando="sh falha.sh", prova="docs")
    with pytest.raises(atualizacoes.PassoFalhou) as e:
        atualizacoes.aplicar_pendentes()
    assert e.value.motivo == "tarefas agendadas: Porta 8765 ocupada por outro processo"


def test_comando_que_falha_nao_marca(passos):
    _escreve(passos, "um", id="um", titulo="Um", comando="exit 3", prova="docs")
    with pytest.raises(atualizacoes.PassoFalhou):
        atualizacoes.aplicar_pendentes()
    assert atualizacoes.aplicados() == set()


def test_prova_que_falha_nao_marca(passos):
    """Comando com exit 0 e efeito ausente é justamente o que a prova existe pra pegar."""
    _escreve(passos, "um", id="um", titulo="Um", comando="echo oi", prova="nao-existe")
    with pytest.raises(atualizacoes.PassoFalhou) as e:
        atualizacoes.aplicar_pendentes()
    assert "nao deixou" in str(e.value)
    assert atualizacoes.aplicados() == set()


def test_prova_nao_passa_pelo_shell(passos):
    """`cmd.exe` não tem `test` nem `true`: a prova é caminho, e caminho existe nos dois sistemas."""
    _escreve(passos, "um", id="um", titulo="Um", comando="echo oi", prova="docs backend/nao-existe")
    with pytest.raises(atualizacoes.PassoFalhou) as e:
        atualizacoes.aplicar_pendentes()
    assert "backend/nao-existe" in str(e.value) and "docs" not in str(e.value).split(":")[-1]


def test_para_no_primeiro_erro(passos, tmp_path):
    """Passo costuma depender do anterior; seguir em frente deixaria estado que ninguém desenhou."""
    depois = tmp_path / "nao-deveria"
    _escreve(passos, "1-quebra", id="1-quebra", titulo="Quebra", comando="exit 1", prova="docs")
    _escreve(passos, "2-depois", id="2-depois", titulo="Depois", comando=f"touch {depois}", prova="docs")
    with pytest.raises(atualizacoes.PassoFalhou):
        atualizacoes.aplicar_pendentes()
    assert not depois.exists()


def test_comando_igual_seguido_roda_uma_vez_e_cada_passo_confere_a_prova(passos, tmp_path):
    """Quatro passos pedindo o instalador inteiro rodavam o instalador quatro vezes."""
    contador = tmp_path / "n"
    instalar = f"echo x >> {contador}"
    _escreve(passos, "1-a", id="1-a", titulo="A", comando=instalar, prova=str(contador))
    _escreve(passos, "2-texto", id="2-texto", titulo="Texto")
    _escreve(passos, "3-b", id="3-b", titulo="B", comando=instalar, prova=str(contador))
    _escreve(passos, "4-outro", id="4-outro", titulo="Outro", comando="echo y", prova="docs")
    _escreve(passos, "5-c", id="5-c", titulo="C", comando=instalar, prova=str(contador))
    _escreve(passos, "6-d", id="6-d", titulo="D", comando=instalar, prova="nao-existe")
    with pytest.raises(atualizacoes.PassoFalhou) as e:
        atualizacoes.aplicar_pendentes()
    # 1-a e 3-b juntos, mesmo com o passo de texto no meio; o comando diferente faz 5-c rodar de novo;
    # 6-d pularia, mas com a prova faltando roda o comando uma vez antes de falhar.
    assert contador.read_text().count("x") == 3
    assert "nao-existe" in str(e.value)
    assert atualizacoes.aplicados() == {"1-a", "2-texto", "3-b", "4-outro", "5-c"}


def test_rodar_duas_vezes_nao_repete(passos, tmp_path):
    contador = tmp_path / "n"
    _escreve(passos, "um", id="um", titulo="Um",
             comando=f"echo x >> {contador}", prova=str(contador))
    atualizacoes.aplicar_pendentes()
    atualizacoes.aplicar_pendentes()
    assert contador.read_text().count("x") == 1


def test_sidecar_e_json_valido(passos):
    _escreve(passos, "um", id="um", titulo="Um")
    atualizacoes.marcar("um")
    assert json.loads(atualizacoes._caminho_aplicados().read_text(encoding="utf-8")) == ["um"]
