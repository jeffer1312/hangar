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
* pasta sem marcador é recusada (senão um `~/.claude-backup` do usuário viraria alvo de poda);
* nada que a conta não criou é seguido: raiz, marcador, gaveta e symlink inesperado como alvo de
  leitura/cópia (tudo isso é deriva → gaveta ou recusa, nunca leitura do alvo externo);
* deriva de memória tem o MESMO tratamento da deriva do topo — gaveta + atalho refeito + aviso,
  e link morto é podado;
* a criação é atômica: falhou no meio (symlink recusado, ~/.claude.json ilegível) → nada sobra.
"""
import ast
import json
import os
import pathlib
import shutil
import subprocess
import sys
import threading

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
    (compartilhado / "CLAUDE.md").write_text("# regras", encoding="utf-8")
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
    assert (p / "CLAUDE.md").is_symlink()


def test_settings_e_copia_e_nao_atalho(casa):
    """O CLI escreve no settings.json por conta própria (primeiro boot, /config, /model). Como
    atalho, essa escrita atravessava e clobberava a config compartilhada de TODAS as contas
    (2026-08-19: 14 chaves sumiram, enabledPlugins junto, plugins desligados em todo lugar)."""
    p = contas.criar("conta2")
    assert not (p / "settings.json").is_symlink()
    assert (p / "settings.json").read_text(encoding="utf-8") == '{"theme":"dark"}'
    # A escrita do CLI na conta fica NA conta — o compartilhado não muda. É esse o sentido do
    # clobber: o que a conta grava nunca sobe. (A volta, o espelho, está no teste seguinte.)
    (p / "settings.json").write_text('{"stripped": true}', encoding="utf-8")
    contas.reconciliar("conta2")
    assert (casa / ".claude" / "settings.json").read_text(encoding="utf-8") == '{"theme":"dark"}'
    assert json.loads((p / "settings.json").read_text(encoding="utf-8")) == {
        "stripped": True, "theme": "dark"}


def test_chaves_do_compartilhado_espelham_pra_conta(casa):
    """A cópia protege do clobber mas tirava a propagação: mexer no principal não chegava nas
    contas. O espelho devolve isso — o principal manda em toda chave que ele tem, e o que só
    existe na cópia da conta fica."""
    p = contas.criar("conta2")
    (casa / ".claude" / "settings.json").write_text(
        '{"theme":"dark","enabledPlugins":{"superpowers@oficial":true}}', encoding="utf-8")
    (p / "settings.json").write_text(
        '{"theme":"light","enabledPlugins":{"velho@x":true},"model":"opus"}', encoding="utf-8")
    contas.reconciliar("conta2")
    d = json.loads((p / "settings.json").read_text(encoding="utf-8"))
    # enabledPlugins espelha POR PLUGIN: o do principal entra, o que só a conta ligou fica.
    assert d["enabledPlugins"] == {"superpowers@oficial": True, "velho@x": True}
    assert d["theme"] == "dark"          # o principal manda: o /config da conta é desfeito
    assert d["model"] == "opus"          # chave que só a conta tem sobrevive


def test_plugin_instalado_de_dentro_da_conta_sobrevive_ao_espelho(casa):
    """`claude plugin install` numa sessão da conta liga o plugin só na cópia dela. Espelhar o
    objeto inteiro desligava no prep seguinte — instalado no compartilhado, ligado em lugar
    nenhum. O principal ainda manda no plugin que conhece: false dele vence true da conta."""
    p = contas.criar("conta2")
    (casa / ".claude" / "settings.json").write_text(
        '{"enabledPlugins":{"ecc@x":true,"humanizer@x":false}}', encoding="utf-8")
    (p / "settings.json").write_text(
        '{"enabledPlugins":{"ecc@x":true,"humanizer@x":true,"novo@x":true}}', encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    d = json.loads((p / "settings.json").read_text(encoding="utf-8"))
    assert d["enabledPlugins"] == {"ecc@x": True, "humanizer@x": False, "novo@x": True}
    assert [a for a in avisos if "enabledPlugins" in a], avisos   # desfez o humanizer: avisa
    # Segunda passada: nada a espelhar (os "plugin X ligado sem instalação" são de outra checagem).
    assert not [a for a in contas.reconciliar("conta2") if "enabledPlugins" in a]


def test_mcp_do_principal_chega_na_conta_antiga(casa):
    """O `.claude.json` era copiado só na criação: MCP adicionado no principal depois nunca
    chegava. Só `mcpServers` espelha — oauthAccount e o resto são da conta."""
    p = contas.criar("conta2")
    (casa / ".claude.json").write_text(json.dumps({
        "oauthAccount": {"emailAddress": "um@exemplo.com"},
        "mcpServers": {"tavily": {"url": "nova"}, "context7": {}}}), encoding="utf-8")
    conta = json.loads((p / ".claude.json").read_text(encoding="utf-8"))
    conta["mcpServers"] = {"tavily": {}, "so-da-conta": {}}
    conta["projects"]["/tmp/x"]["allowedTools"] = ["Bash", "Edit"]
    (p / ".claude.json").write_text(json.dumps(conta), encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    d = json.loads((p / ".claude.json").read_text(encoding="utf-8"))
    assert d["mcpServers"] == {"tavily": {"url": "nova"}, "context7": {}, "so-da-conta": {}}
    assert "oauthAccount" not in d
    assert d["projects"]["/tmp/x"]["allowedTools"] == ["Bash", "Edit"]   # estado da conta fica
    assert [a for a in avisos if "tavily" in a], avisos
    assert not contas.reconciliar("conta2")


def test_espelho_avisa_quando_desfaz_chave_da_conta(casa):
    """O principal manda, mas desfazer o /model da conta sem rastro deixava o modelo mudar
    sozinho na abertura sem ninguém saber por quê. Chave só ADICIONADA não vira aviso."""
    p = contas.criar("conta2")
    (casa / ".claude" / "settings.json").write_text(
        '{"theme":"dark","outputStyle":"Concise"}', encoding="utf-8")
    (p / "settings.json").write_text('{"theme":"light"}', encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert [a for a in avisos if "theme" in a], avisos
    assert not [a for a in avisos if "outputStyle" in a], avisos
    assert not contas.reconciliar("conta2")   # nada mudou: sem aviso repetido


def test_espelhamento_nao_apaga_chave_que_o_compartilhado_perdeu(casa):
    """Ausência da chave no compartilhado é o sintoma do acidente de 2026-08-19 (clobber);
    espelhar a ausência desligaria os plugins das contas de novo."""
    p = contas.criar("conta2")
    (p / "settings.json").write_text(
        '{"enabledPlugins":{"superpowers@oficial":true}}', encoding="utf-8")
    contas.reconciliar("conta2")   # compartilhado da fixture não tem enabledPlugins
    d = json.loads((p / "settings.json").read_text(encoding="utf-8"))
    assert d["enabledPlugins"] == {"superpowers@oficial": True}


def test_settings_symlink_do_layout_antigo_vira_copia(casa):
    """Conta criada antes da mudança tem settings.json como atalho; o próximo uso troca por
    cópia sem ninguém rodar nada à mão — o mesmo contrato do resto da reconciliação."""
    p = contas.criar("conta2")
    (p / "settings.json").unlink()
    os.symlink(casa / ".claude" / "settings.json", p / "settings.json")
    contas.reconciliar("conta2")
    assert not (p / "settings.json").is_symlink()
    assert (p / "settings.json").read_text(encoding="utf-8") == '{"theme":"dark"}'


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
    (casa / ".claude" / "CLAUDE.md").unlink()
    contas.reconciliar("conta2")
    assert not (casa / ".claude-conta2" / "CLAUDE.md").is_symlink()


def test_arquivo_local_alterado_volta_pro_compartilhado(casa):
    """Quem grava por tmp+rename substitui o ATALHO por arquivo comum dentro da conta. Descartar
    perderia a mudança nas duas contas, e o único rastro seria um log que ninguém lê. Como o
    arquivo é compartilhado por desenho, a mudança sobe pro compartilhado."""
    p = contas.criar("conta2")
    (p / "CLAUDE.md").unlink()
    (p / "CLAUDE.md").write_text("# regras editadas", encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert (casa / ".claude" / "CLAUDE.md").read_text(encoding="utf-8") == "# regras editadas"
    assert (p / "CLAUDE.md").is_symlink()
    assert any("CLAUDE.md" in a for a in avisos)


def test_arquivo_local_identico_nao_gera_aviso(casa):
    p = contas.criar("conta2")
    (p / "CLAUDE.md").unlink()
    (p / "CLAUDE.md").write_text("# regras", encoding="utf-8")
    assert contas.reconciliar("conta2") == []
    assert (p / "CLAUDE.md").is_symlink()


def test_pasta_local_vai_pra_drift_com_teto(casa):
    """A gaveta guarda as DRIFT_TETO mais novas com o CONTEÚDO dentro — três pastas vazias
    satisfariam `len == DRIFT_TETO` sem preservar nada. O mtime é controlado porque sem isto a
    ordem das entradas é a do relógio e o teste não saberia dizer quais são as mais novas."""
    p = contas.criar("conta2")
    for i in range(5):
        (p / "skills").unlink()
        (p / "skills").mkdir()
        (p / "skills" / "versao.txt").write_text(str(i), encoding="utf-8")
        os.utime(p / "skills", (1_700_000_000 + i, 1_700_000_000 + i))
        contas.reconciliar("conta2")
    entradas = list((p / ".drift").iterdir())
    assert len(entradas) == contas.DRIFT_TETO
    versoes = {(p / ".drift" / e.name / "versao.txt").read_text(encoding="utf-8")
               for e in entradas}
    assert versoes == {"2", "3", "4"}


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


def test_nome_com_quebra_de_linha_e_recusado(casa):
    """re.match com $ casa antes do \n final — fullmatch não. Sem esta guarda, a pasta nasceria
    com quebra de linha no nome, e label, caminho e logs ganhariam controle de linha."""
    for nome in ("conta2\n", "conta/2", "..", "x" * 33):
        with pytest.raises(contas.ContaError):
            contas.criar(nome)
    assert not (casa / ".claude-conta2\n").exists()


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


def test_arquivo_local_contra_pasta_compartilhada_vai_pra_gaveta(casa):
    """Pasta que apareceu no ~/.claude depois (aqui: hooks/) colide com arquivo local de mesmo
    nome na conta. Fundir não dá; o arquivo vai pra gaveta COM o conteúdo — não some."""
    p = contas.criar("conta2")
    (p / "hooks").write_text("dado do usuário", encoding="utf-8")
    (casa / ".claude" / "hooks").mkdir()
    avisos = contas.reconciliar("conta2")
    assert (p / "hooks").is_symlink()
    assert (casa / ".claude" / "hooks").is_dir()
    gavetas = list((p / ".drift").iterdir())
    assert len(gavetas) == 1
    assert gavetas[0].read_text(encoding="utf-8") == "dado do usuário"
    assert any("hooks" in a for a in avisos)


def test_symlink_local_inesperado_vai_pra_gaveta_sem_ler_o_alvo(casa):
    """Symlink que não é o atalho esperado dentro da conta aponta pra onde quiser. Seguir o link
    (filecmp/copyfile lendo o alvo externo) vazaria o conteúdo dele pro compartilhado; a gaveta
    preserva o link sem ler nada."""
    p = contas.criar("conta2")
    externo = casa / "segredo.txt"
    externo.write_text("dado externo", encoding="utf-8")
    (p / "CLAUDE.md").unlink()
    os.symlink(externo, p / "CLAUDE.md")
    avisos = contas.reconciliar("conta2")
    assert (casa / ".claude" / "CLAUDE.md").read_text(encoding="utf-8") == "# regras"
    assert (p / "CLAUDE.md").is_symlink()
    assert (p / "CLAUDE.md").read_text(encoding="utf-8") == "# regras"
    assert any("CLAUDE.md" in a for a in avisos)


def test_drift_symlinkado_recusa_sem_tocar_o_alvo(casa):
    """`.drift` é caminho interno da conta. Symlinkado pra fora, a gaveta passaria a mover — e a
    poda, a apagar — arquivos no diretório externo. Recusa com erro claro."""
    p = contas.criar("conta2")
    fora = casa / "fora"
    fora.mkdir()
    sentinela = fora / "sentinela.txt"
    sentinela.write_text("intacto", encoding="utf-8")
    os.symlink(fora, p / ".drift")
    (p / "skills").unlink()
    (p / "skills").mkdir()
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("conta2")
    assert e.value.status == 500
    assert sentinela.read_text(encoding="utf-8") == "intacto"
    assert not (fora / "skills.1").exists()


def test_pasta_raiz_symlinkada_nao_e_conta(casa):
    """~/.claude-evil como symlink pra fora com um marcador do lado de lá: sem a guarda, listar()
    exibiria 'evil' e reconciliar/apagar remexeriam o diretório externo."""
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    os.symlink(fora, casa / ".claude-evil")
    assert contas.listar() == []
    for op in (contas.reconciliar, contas.apagar):
        with pytest.raises(contas.ContaError) as e:
            op("evil")
        assert e.value.status == 404


def test_reconciliar_recusa_projeto_fora_da_arvore(casa):
    """`projeto` entra em caminhos (raiz / projeto / memory): absoluto, `..` ou barra escapariam
    do ~/.claude/projects. O regex aceita exatamente o que registry.sanitize_cwd produz e
    rejeita o resto, ANTES de montar qualquer Path."""
    contas.criar("conta2")
    for mal in ("/tmp/fora", "../../fora", "a/b", "a\\b"):
        with pytest.raises(contas.ContaError) as e:
            contas.reconciliar("conta2", projeto=mal)
        assert e.value.status == 400
    contas.reconciliar("conta2", projeto="-home-jefferson-novo")
    assert (casa / ".claude-conta2" / "projects" / "-home-jefferson-novo" / "memory").is_symlink()


def test_reconciliacao_concorrente_de_contas_diferentes(casa):
    """Duas contas com versões DIFERENTES do mesmo arquivo reconciliando ao mesmo tempo: a trava
    compartilhada serializa as escritas no ~/.claude — o arquivo final é uma das versões
    íntegras, as duas contas terminam religadas e as duas mudanças são reportadas (sem a trava,
    os copyfile se atropelam no meio do caminho)."""
    contas.criar("conta_a")
    contas.criar("conta_b")
    versoes = {"conta_a": "# regras da a", "conta_b": "# regras da b"}
    for nome, versao in versoes.items():
        cfg = casa / f".claude-{nome}" / "CLAUDE.md"
        cfg.unlink()
        cfg.write_text(versao, encoding="utf-8")
    barreira = threading.Barrier(2)
    avisos: list[list[str]] = []

    def roda(nome: str) -> None:
        barreira.wait()
        avisos.append(contas.reconciliar(nome))

    t1 = threading.Thread(target=roda, args=("conta_a",))
    t2 = threading.Thread(target=roda, args=("conta_b",))
    t1.start()
    t2.start()
    t1.join()
    t2.join()

    final = (casa / ".claude" / "CLAUDE.md").read_text(encoding="utf-8")
    assert final in set(versoes.values())
    for nome in ("conta_a", "conta_b"):
        assert (casa / f".claude-{nome}" / "CLAUDE.md").is_symlink()
    assert len(avisos) == 2
    for a in avisos:
        assert any("CLAUDE.md" in x for x in a)


def test_criar_sem_claude_na_home_cria_o_compartilhado(casa):
    """Máquina nova não tem ~/.claude. Sem o mkdir do compartilhado, o reconciliar do criar()
    morreria no meio e a conta parcial carimbada sobraria — aparecendo no seletor e travando o
    cadastro com 409. Hoje a conta nasce completa e o ~/.claude nasce junto."""
    shutil.rmtree(casa / ".claude")
    p = contas.criar("nova")
    assert (p / contas.MARCADOR).is_file()
    assert (p / "projects").is_dir()
    assert (casa / ".claude" / "projects").is_dir()
    assert contas.listar() == ["nova"]


def test_criar_com_falha_nao_deixa_conta_parcial(casa, monkeypatch):
    """Falha no meio da criação (aqui: symlink recusado, como no Windows sem Modo
    Desenvolvedor) não pode deixar a pasta carimbada: ela apareceria no seletor e travaria o
    cadastro com 409 pra sempre."""
    def sem_symlink(*args, **kwargs):
        raise OSError("sem Modo Desenvolvedor")
    monkeypatch.setattr(os, "symlink", sem_symlink)
    with pytest.raises(contas.ContaError) as e:
        contas.criar("nova")
    assert e.value.status == 500
    assert not (casa / ".claude-nova").exists()
    assert contas.listar() == []


def test_memoria_local_vai_pra_gaveta_e_atalho_e_refeito(casa):
    """Memória local de verdade (pasta com MEMORY.md — quem grava por tmp+rename substitui o
    atalho, ou um claude rodado direto na conta cria a pasta) não pode ficar quieta: esta conta
    guardaria uma memória que nenhuma outra vê. Vai pra gaveta e o atalho é refeito, com aviso."""
    p = contas.criar("conta2")
    memo = p / "projects" / "-tmp-x" / "memory"
    memo.unlink()
    memo.mkdir()
    (memo / "MEMORY.md").write_text("local", encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert memo.is_symlink()
    assert (memo / "MEMORY.md").read_text(encoding="utf-8") == "m"
    assert any("memory" in a for a in avisos)
    gavetas = list((p / ".drift").iterdir())
    assert len(gavetas) == 1
    assert (gavetas[0] / "MEMORY.md").read_text(encoding="utf-8") == "local"


def test_memoria_com_projeto_removido_do_compartilhado_e_podada(casa):
    """Projeto que sumiu do ~/.claude deixa o atalho de memória quebrado. A poda do topo da
    reconciliar não alcança projects/ — sem esta passada, o link morto ficaria pra sempre."""
    p = contas.criar("conta2")
    shutil.rmtree(casa / ".claude" / "projects" / "-tmp-x")
    contas.reconciliar("conta2")
    assert not (p / "projects" / "-tmp-x" / "memory").is_symlink()


def test_criar_com_claude_json_truncado_falha_sem_conta_parcial(casa):
    """~/.claude.json truncado por uma escrita concorrente não pode virar conta com cara de
    sucesso e configuração vazia — o erro sobe e a conta parcial some (rollback do criar)."""
    (casa / ".claude.json").write_text('{"oauthAccount": {"emailA', encoding="utf-8")
    with pytest.raises(contas.ContaError) as e:
        contas.criar("conta2")
    assert e.value.status == 500
    assert not (casa / ".claude-conta2").exists()


def test_criar_com_claude_json_nao_objeto_falha(casa):
    (casa / ".claude.json").write_text("[1, 2, 3]", encoding="utf-8")
    with pytest.raises(contas.ContaError) as e:
        contas.criar("conta2")
    assert e.value.status == 500
    assert not (casa / ".claude-conta2").exists()


@pytest.mark.skipif(os.name != "posix",
                    reason="chmod(0) e no-op no Windows: o arquivo segue legivel (quem nega e a ACL)")
def test_criar_com_claude_json_sem_permissao_falha(casa):
    (casa / ".claude.json").chmod(0)
    try:
        with pytest.raises(contas.ContaError) as e:
            contas.criar("conta2")
        assert e.value.status == 500
        assert not (casa / ".claude-conta2").exists()
    finally:
        (casa / ".claude.json").chmod(0o644)


def test_arquivo_colidindo_com_nome_temporario_nao_e_apagado(casa):
    """O temporário do _ligar não pode ter nome fixo: um arquivo legítimo `skills.hangar-novo`
    na conta seria apagado na próxima reconciliação. Com nome único (pid+uuid) ele sobrevive."""
    p = contas.criar("conta2")
    (p / "skills").unlink()
    (p / "skills.hangar-novo").write_text("dado do usuário", encoding="utf-8")
    contas.reconciliar("conta2")
    assert (p / "skills.hangar-novo").read_text(encoding="utf-8") == "dado do usuário"
    assert (p / "skills").is_symlink()


def test_modulo_e_stdlib_pura():
    """O hangar-conta importa este módulo com o python3 do SISTEMA (sem venv). A prova é o import
    num interpretador SEM site-packages (`-S`): qualquer dependência não-stdlib falharia ali,
    com o rastro do culpado no stderr. A varredura de AST fica como diagnóstico rápido."""
    raiz = pathlib.Path(contas.__file__).resolve().parent.parent
    proc = subprocess.run(
        [sys.executable, "-S", "-c",
         "import sys; sys.path.insert(0, sys.argv[1]); import app.contas", str(raiz)],
        capture_output=True, text=True)
    assert proc.returncode == 0, proc.stderr
    fonte = pathlib.Path(contas.__file__).read_text(encoding="utf-8")
    arvore = ast.parse(fonte)
    importados = {
        n.module.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.ImportFrom) and n.module
    } | {
        a.name.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.Import) for a in n.names
    }
    assert not (importados & {"pydantic", "fastapi", "httpx"})
    # `app` saiu do bloqueio em bloco e virou allowlist, pelo mesmo motivo do test_engines: o
    # `app.atomico` e stdlib puro e existe pra nao duplicar a retentativa do `os.replace` que o
    # Windows exige. Quem prova de verdade e o import com `-S` la em cima — ele carregou
    # `app.contas` inteiro sem site-packages, `atomico` junto. A allowlist so mantem o
    # diagnostico rapido honesto: nome novo aqui tem que ser stdlib puro tambem.
    de_app = {a.name for n in ast.walk(arvore)
              if isinstance(n, ast.ImportFrom) and n.module == "app" for a in n.names}
    assert de_app <= {"atomico"}


def test_drift_poda_por_nome_e_nao_pela_gaveta_inteira(casa):
    """A gaveta existe pra NAO perder dado — o teto tem que ser POR ARQUIVO DE ORIGEM.

    Com `iterdir()` cru, DRIFT_TETO versoes de `skills` enchiam a gaveta e a poda levava junto o
    `plugins.1`, que era a UNICA copia de uma pasta que o usuario tinha editado a mao. E calado: o
    aviso devolvido so fala da pasta que esta ENTRANDO, nunca da que foi expulsa.
    """
    compartilhado = pathlib.Path(os.environ["HOME"]) / ".claude"
    (compartilhado / "plugins").mkdir()
    contas.criar("gaveta")
    dir_conta = pathlib.Path(os.environ["HOME"]) / ".claude-gaveta"

    # plugins vira pasta LOCAL, com conteudo que so existe aqui -> vai pra .drift/plugins.1
    (dir_conta / "plugins").unlink()
    (dir_conta / "plugins").mkdir()
    (dir_conta / "plugins" / "meu.json").write_text("unico", encoding="utf-8")
    contas.reconciliar("gaveta")
    guardado = dir_conta / ".drift" / "plugins.1" / "meu.json"
    assert guardado.is_file(), "a colisao de pasta local devia ter ido pra gaveta"

    # agora DRIFT_TETO+1 colisoes de OUTRO nome, o suficiente pra encher o teto global
    for i in range(contas.DRIFT_TETO + 1):
        (dir_conta / "skills").unlink()
        (dir_conta / "skills").mkdir()
        (dir_conta / "skills" / f"v{i}.md").write_text(str(i), encoding="utf-8")
        contas.reconciliar("gaveta")

    assert guardado.is_file(), "a poda de 'skills' levou o unico backup de 'plugins' junto"
    assert guardado.read_text(encoding="utf-8") == "unico"
    # e o teto continua valendo DENTRO do proprio nome
    skills = [p for p in (dir_conta / ".drift").iterdir() if p.name.startswith("skills.")]
    assert len(skills) <= contas.DRIFT_TETO


def test_apelidos_nao_viram_atalho_nem_sobem_de_copia_velha(casa):
    """O arquivo de apelidos é do APP, lido/gravado só pelo caminho compartilhado. Symlink dentro
    da conta não serve pra nada, e uma cópia real antiga fazia _resolver_colisao copiá-la POR CIMA
    do compartilhado — foi o que apagou os apelidos em 19/08 (mesma janela do settings.json)."""
    apelidos = casa / ".claude" / ".hangar-apelidos.json"
    apelidos.write_text('{"claude:/x": "Nome Bom"}', encoding="utf-8")
    p = contas.criar("conta2")
    assert not (p / ".hangar-apelidos.json").exists()          # nem atalho, nem cópia
    # Cópia VELHA deixada na conta não pode subir pro compartilhado na reconciliação.
    (p / ".hangar-apelidos.json").write_text('{"claude:/x": "velho"}', encoding="utf-8")
    contas.reconciliar("conta2")
    assert json.loads(apelidos.read_text(encoding="utf-8")) == {"claude:/x": "Nome Bom"}


def test_plugin_ligado_sem_instalacao_gera_aviso(casa):
    """`enabledPlugins` (cópia da conta) e `installed_plugins.json` (compartilhado) desalinham
    calados: o clobber de 2026-08-19 apagou a instalação e o true ficou; a limpeza de cache do
    CLI apaga a pasta de plugin que saiu do registro. A sessão abre sem o plugin e nada avisa."""
    p = contas.criar("conta2")
    pasta = casa / ".claude" / "plugins" / "cache" / "oficial" / "ecc" / "1.0"
    pasta.mkdir(parents=True)
    (casa / ".claude" / "plugins" / "installed_plugins.json").write_text(json.dumps({
        "version": 2,
        "plugins": {
            "ecc@oficial": [{"scope": "user", "installPath": str(pasta)}],
            "sumiu@oficial": [{"scope": "user", "installPath": str(pasta.parent / "nada")}],
        }}), encoding="utf-8")
    (casa / ".claude" / "settings.json").write_text(json.dumps({"enabledPlugins": {
        "ecc@oficial": True, "sumiu@oficial": True,
        "superpowers@oficial": True, "desligado@oficial": False}}), encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert [a for a in avisos if "superpowers@oficial" in a and "não consta" in a], avisos
    assert [a for a in avisos if "sumiu@oficial" in a and "pasta sumiu" in a], avisos
    assert not [a for a in avisos if "ecc@oficial" in a or "desligado" in a], avisos


def test_sem_registro_de_plugins_e_sem_plugin_ligado_nao_avisa(casa):
    """Máquina nova: nem installed_plugins.json nem enabledPlugins. Silêncio, não erro."""
    contas.criar("conta2")
    assert not [a for a in contas.reconciliar("conta2") if a.startswith("plugin")]
