"""Contas Claude: uma pasta por conta, com o ambiente compartilhado por atalho.

O que esta suíte trava:

* a conta nasce com marcador e com o `.claude.json` SEM o `oauthAccount` (deixá-lo faria o CLI
  abrir dizendo estar logado numa conta cujo token ele não tem);
* `projects/` é pasta REAL — compartilhá-la faria o painel de custo contar o mesmo `.jsonl` uma
  vez por conta (ver `costs_sources._config_dirs`), e o número errado sairia calado;
* dentro dela, `memory/` é atalho — memória não custa e vale nas três contas;
* reconciliar é idempotente e pega pasta que apareceu no `~/.claude` DEPOIS (é o que impede a
  deriva);
* arquivo de topo que virou local com conteúdo NOVO é devolvido pro compartilhado, não descartado
  (quem grava por tmp+rename substitui o atalho por arquivo comum: mandar pra `.drift` perderia a
  mudança nas duas contas);
* pasta sem marcador é recusada (senão um `~/.claude-backup` do usuário viraria alvo de poda).
"""
import ast
import json
import pathlib

import pytest

from app import contas


@pytest.fixture
def casa(tmp_path, monkeypatch):
    """HOME isolado com um ~/.claude e um ~/.claude.json plausíveis."""
    monkeypatch.setenv("HOME", str(tmp_path))
    compartilhado = tmp_path / ".claude"
    (compartilhado / "skills" / "falar").mkdir(parents=True)
    (compartilhado / "skills" / "falar" / "SKILL.md").write_text("oi", encoding="utf-8")
    (compartilhado / "projects" / "-tmp-x" / "memory").mkdir(parents=True)
    (compartilhado / "projects" / "-tmp-x" / "memory" / "MEMORY.md").write_text("m", encoding="utf-8")
    (compartilhado / "settings.json").write_text('{"theme":"dark"}', encoding="utf-8")
    (compartilhado / ".credentials.json").write_text('{"claudeAiOauth":{}}', encoding="utf-8")
    (tmp_path / ".claude.json").write_text(
        json.dumps({"oauthAccount": {"emailAddress": "um@exemplo.com"},
                    "mcpServers": {"tavily": {}},
                    "projects": {"/tmp/x": {"allowedTools": ["Bash"]}}}),
        encoding="utf-8")
    return tmp_path


def test_criar_deixa_a_conta_com_marcador(casa):
    p = contas.criar("conta2")
    assert p == casa / ".claude-conta2"
    assert (p / contas.MARCADOR).is_file()


def test_criar_semeia_claude_json_sem_a_conta_de_origem(casa):
    """O oauthAccount é o único campo que PRECISA ser diferente. O resto é copiado de propósito:
    as permissões já aceitas por diretório e os MCP de escopo usuário moram nesse arquivo."""
    p = contas.criar("conta2")
    d = json.loads((p / ".claude.json").read_text(encoding="utf-8"))
    assert "oauthAccount" not in d
    assert d["mcpServers"] == {"tavily": {}}
    assert d["projects"] == {"/tmp/x": {"allowedTools": ["Bash"]}}


def test_credenciais_nao_viram_atalho(casa):
    """São a identidade da conta: como atalho, as duas dividiriam o mesmo token e a renovação de
    uma derrubaria a outra — o motivo inteiro de existir uma pasta por conta."""
    p = contas.criar("conta2")
    assert not (p / ".credentials.json").exists()
    assert not (p / ".claude.json").is_symlink()


def test_projects_e_pasta_real_e_nao_atalho(casa):
    """Compartilhar `projects/` faria o painel somar o mesmo .jsonl uma vez por conta —
    gasto multiplicado, e ainda aparecendo em conta que nunca rodou nada."""
    p = contas.criar("conta2")
    assert (p / "projects").is_dir()
    assert not (p / "projects").is_symlink()


def test_memoria_do_projeto_e_atalho_pro_compartilhado(casa):
    p = contas.criar("conta2")
    memo = p / "projects" / "-tmp-x" / "memory"
    assert memo.is_symlink()
    assert (memo / "MEMORY.md").read_text(encoding="utf-8") == "m"


def test_memoria_de_projeto_novo_e_criada_sob_demanda(casa):
    """Projeto que ninguém abriu ainda não tem memória compartilhada pra ligar. Passar o projeto
    na reconciliação cria a pasta no compartilhado e liga — é o caminho do backend, que sabe o
    cwd da sessão que está subindo."""
    contas.criar("conta2")
    contas.reconciliar("conta2", projeto="-home-jefferson-novo")
    memo = casa / ".claude-conta2" / "projects" / "-home-jefferson-novo" / "memory"
    assert memo.is_symlink()
    assert (casa / ".claude" / "projects" / "-home-jefferson-novo" / "memory").is_dir()


def test_o_resto_do_ambiente_e_atalho(casa):
    p = contas.criar("conta2")
    assert (p / "skills").is_symlink()
    assert (p / "skills" / "falar" / "SKILL.md").read_text(encoding="utf-8") == "oi"
    assert (p / "settings.json").is_symlink()


def test_reconciliar_pega_pasta_que_apareceu_depois(casa):
    """A deriva que este projeto precisa evitar: pasta nova no ~/.claude não existiria na conta, e
    o CLI criaria uma LOCAL ali, calado."""
    contas.criar("conta2")
    (casa / ".claude" / "plugins").mkdir()
    contas.reconciliar("conta2")
    assert (casa / ".claude-conta2" / "plugins").is_symlink()


def test_reconciliar_e_idempotente(casa):
    contas.criar("conta2")
    assert contas.reconciliar("conta2") == []
    assert contas.reconciliar("conta2") == []


def test_reconciliar_poda_atalho_pro_que_sumiu(casa):
    contas.criar("conta2")
    (casa / ".claude" / "settings.json").unlink()
    contas.reconciliar("conta2")
    assert not (casa / ".claude-conta2" / "settings.json").is_symlink()


def test_arquivo_local_alterado_volta_pro_compartilhado(casa):
    """Quem grava por tmp+rename substitui o ATALHO por arquivo comum dentro da conta. Descartar
    perderia a mudança nas duas contas, e o único rastro seria um log que ninguém lê. Como o
    arquivo é compartilhado por desenho, a mudança sobe pro compartilhado."""
    p = contas.criar("conta2")
    (p / "settings.json").unlink()
    (p / "settings.json").write_text('{"theme":"light"}', encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert (casa / ".claude" / "settings.json").read_text(encoding="utf-8") == '{"theme":"light"}'
    assert (p / "settings.json").is_symlink()
    assert any("settings.json" in a for a in avisos)


def test_arquivo_local_identico_nao_gera_aviso(casa):
    p = contas.criar("conta2")
    (p / "settings.json").unlink()
    (p / "settings.json").write_text('{"theme":"dark"}', encoding="utf-8")
    assert contas.reconciliar("conta2") == []
    assert (p / "settings.json").is_symlink()


def test_pasta_local_vai_pra_drift_com_teto(casa):
    """Pasta não dá pra fundir com o compartilhado — vai pra gaveta. Com teto, senão a gaveta
    cresce pra sempre sem ninguém olhar."""
    p = contas.criar("conta2")
    for _ in range(5):
        (p / "skills").unlink()
        (p / "skills").mkdir()
        contas.reconciliar("conta2")
    assert len(list((p / ".drift").iterdir())) == contas.DRIFT_TETO


def test_reconciliar_recusa_pasta_sem_marcador(casa):
    """Sem esta guarda, um ~/.claude-backup do usuário viraria alvo de poda e de .drift."""
    (casa / ".claude-backup").mkdir()
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("backup")
    assert e.value.status == 404


def test_criar_em_cima_de_pasta_existente_estoura(casa):
    (casa / ".claude-conta2").mkdir()
    with pytest.raises(contas.ContaError) as e:
        contas.criar("conta2")
    assert e.value.status == 409


def test_nome_invalido_estoura(casa):
    with pytest.raises(contas.ContaError):
        contas.criar("Conta 2")


def test_apagar_so_aceita_pasta_carimbada(casa):
    (casa / ".claude-backup").mkdir()
    with pytest.raises(contas.ContaError) as e:
        contas.apagar("backup")
    assert e.value.status == 404
    assert (casa / ".claude-backup").is_dir()


def test_apagar_remove_a_conta(casa):
    """Digitou o nome errado no cadastro? Sem isto a pasta fica pra sempre no seletor."""
    contas.criar("cotna2")
    contas.apagar("cotna2")
    assert not (casa / ".claude-cotna2").exists()


def test_listar_devolve_so_conta_carimbada(casa):
    contas.criar("conta2")
    (casa / ".claude-backup").mkdir()
    assert contas.listar() == ["conta2"]


def test_modulo_e_stdlib_pura():
    """O cp-conta importa este módulo com o python3 do SISTEMA (sem venv). Um import de app.config
    puxaria pydantic e quebraria o terminal deixando só o app funcionando — falha assimétrica,
    chata de diagnosticar. Sentinela, não prova: barra os culpados conhecidos."""
    fonte = pathlib.Path(contas.__file__).read_text(encoding="utf-8")
    arvore = ast.parse(fonte)
    importados = {
        n.module.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.ImportFrom) and n.module
    } | {
        a.name.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.Import) for a in n.names
    }
    assert not (importados & {"app", "pydantic", "fastapi", "httpx"})
