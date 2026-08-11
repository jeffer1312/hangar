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
* nada que não seja dado da conta é seguido: symlink apontando pra fora, raiz de conta symlinkada,
  gaveta `.drift` symlinkada e `projeto` com `..`/barra são recusados ou vão pra gaveta sem tocar
  no alvo;
* criação com falha no meio (sem `~/.claude`, symlink recusado) NÃO deixa conta parcial;
* reconciliações de contas diferentes se serializam — o `~/.claude` compartilhado é escrito por
  todas, e o último-vencedor de uma corrida perderia alteração sem aviso;
* o módulo é importável com o python do sistema (`-S`, sem site-packages) — é o que o `cp-conta`
  usa.
"""
import ast
import json
import os
import pathlib
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
    cresce pra sempre sem ninguém olhar. Cada versão carrega conteúdo próprio e mtime controlado:
    o teste trava a ORDEM da poda, não só a contagem — apagar tudo e criar três pastas vazias
    passaria na contagem, não aqui."""
    p = contas.criar("conta2")
    base = 1_700_000_000
    for i in range(5):
        (p / "skills").unlink()
        (p / "skills").mkdir()
        (p / "skills" / "versao.txt").write_text(str(i), encoding="utf-8")
        os.utime(p / "skills", (base + i, base + i))
        contas.reconciliar("conta2")
    gaveta = p / ".drift"
    assert len(list(gaveta.iterdir())) == contas.DRIFT_TETO
    conteudos = {int((gaveta / f"skills.{n}" / "versao.txt").read_text(encoding="utf-8"))
                 for n in range(1, 6) if (gaveta / f"skills.{n}").is_dir()}
    assert conteudos == {2, 3, 4}


def test_arquivo_local_contra_alvo_que_virou_pasta_vai_pra_gaveta(casa):
    """`hooks` virou pasta no ~/.claude e a conta tem um arquivo local `hooks` com dados:
    descartar perderia o arquivo. Incompatibilidade de tipo é deriva — vai pra gaveta, íntegro."""
    p = contas.criar("conta2")
    (casa / ".claude" / "hooks").mkdir()   # pasta no compartilhado, criada DEPOIS da conta
    (p / "hooks").write_text("script meu", encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert (p / "hooks").is_symlink()
    assert any("hooks" in a for a in avisos)
    assert (p / ".drift" / "hooks.1").read_text(encoding="utf-8") == "script meu"


def test_symlink_local_inesperado_vai_pra_gaveta_sem_ler_o_alvo(casa):
    """Symlink dentro da conta apontando pra fora não pode ser seguido: filecmp/copyfile leriam o
    alvo externo e sobrescreveriam o compartilhado com ele. O link vai pra gaveta sem tocar no
    alvo — o compartilhado fica como está."""
    p = contas.criar("conta2")
    segredo = casa / "segredo.txt"
    segredo.write_text("s3nh4", encoding="utf-8")
    (p / "settings.json").unlink()
    (p / "settings.json").symlink_to(segredo)
    avisos = contas.reconciliar("conta2")
    assert (casa / ".claude" / "settings.json").read_text(encoding="utf-8") == '{"theme":"dark"}'
    assert os.readlink(p / "settings.json") == str(casa / ".claude" / "settings.json")
    assert any("settings.json" in a for a in avisos)
    assert (p / ".drift" / "settings.json.1").is_symlink()


def test_gaveta_drift_symlinkada_e_recusada(casa):
    """Gaveta .drift apontando pra fora: o move mandaria a pasta colidida pro alvo externo, e a
    poda poderia rmtree lá. Recusar é a única saída que não mexe fora da conta."""
    p = contas.criar("conta2")
    fora = casa / "fora"
    fora.mkdir()
    (p / "skills").unlink()
    (p / "skills").mkdir()
    (p / ".drift").symlink_to(fora, target_is_directory=True)
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("conta2")
    assert e.value.status == 500
    assert not (fora / "skills.1").exists()


def test_conta_raiz_symlinkada_e_ignorada(casa):
    """`~/.claude-evil -> /tmp/fora` com um marcador plantado lá dentro não pode virar conta:
    listar mostraria a conta, reconciliar remexeria em /tmp/fora e apagar derrubaria com erro
    bruto de rmtree em symlink."""
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    (casa / ".claude-evil").symlink_to(fora, target_is_directory=True)
    assert contas.e_conta(casa / ".claude-evil") is False
    assert "evil" not in contas.listar()
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("evil")
    assert e.value.status == 404
    with pytest.raises(contas.ContaError) as e:
        contas.apagar("evil")
    assert e.value.status == 404


def test_projeto_absoluto_ou_com_traversal_e_recusado(casa):
    """`projeto` é entrada da interface pública e vira caminho: absoluto, `..`, barra e barra
    invertida escapariam de ~/.claude. A regra aceita exatamente o que registry.sanitize_cwd
    produz (letras, dígitos e hífen)."""
    contas.criar("conta2")
    for projeto in ("/tmp/fora", "../../fora", "a/b", "a\\b"):
        with pytest.raises(contas.ContaError) as e:
            contas.reconciliar("conta2", projeto=projeto)
        assert e.value.status == 400
    assert not (casa / "tmp" / "fora").exists()
    assert not (casa / "fora").exists()


def test_reconciliacoes_paralelas_de_contas_diferentes_nao_se_perdem(casa):
    """Duas contas reconciliando ao mesmo tempo escrevem no MESMO ~/.claude: o settings.json
    alterado dentro de cada conta sobe pro compartilhado. Sem a trava compartilhada, as duas
    passam o filecmp contra o mesmo estado e o último copyfile vence — a alteração da primeira
    morre sem aviso. A corrida real não reproduz com determinismo (janela pequena), então o
    teste roda a condição 20 vezes e verifica os invariantes: nenhum erro, o compartilhado
    sempre termina com UMA das versões íntegras e as duas contas religadas."""
    contas.criar("uma")
    contas.criar("dois")
    erros: list[Exception] = []

    def rodar(nome: str) -> None:
        try:
            contas.reconciliar(nome)
        except Exception as e:  # noqa: BLE001 — o assert fora da thread é o teste
            erros.append(e)

    versoes = {"uma": "light", "dois": "dark"}
    for _ in range(20):
        # O compartilhado parte de uma TERCEIRA versão: as duas contas têm alteração local a
        # subir, que é exatamente a condição da corrida (ambas passam o filecmp contra o mesmo).
        (casa / ".claude" / "settings.json").write_text('{"theme":"grey"}', encoding="utf-8")
        for nome, tema in versoes.items():
            alvo = casa / f".claude-{nome}" / "settings.json"
            alvo.unlink()
            alvo.write_text(json.dumps({"theme": tema}), encoding="utf-8")
        t1 = threading.Thread(target=rodar, args=("uma",))
        t2 = threading.Thread(target=rodar, args=("dois",))
        t1.start()
        t2.start()
        t1.join()
        t2.join()
        assert not erros, erros
        final = json.loads((casa / ".claude" / "settings.json").read_text(encoding="utf-8"))
        assert final["theme"] in versoes.values()
        for nome in versoes:
            assert (casa / f".claude-{nome}" / "settings.json").is_symlink()


def test_criar_sem_claude_na_maquina(casa):
    """Máquina nova: nem ~/.claude nem ~/.claude.json existem. A conta nasce mesmo assim — sem o
    mkdir explícito do compartilhado, criar() quebrava no iterdir() da reconciliação e ainda
    deixava pasta parcial com marcador: conta quebrada no seletor e 409 na tentativa seguinte."""
    import shutil
    shutil.rmtree(casa / ".claude")
    (casa / ".claude.json").unlink()
    p = contas.criar("conta2")
    assert (casa / ".claude").is_dir()
    assert (p / contas.MARCADOR).is_file()
    assert contas.listar() == ["conta2"]


def test_falha_ao_criar_atalho_nao_deixa_conta_parcial(casa, monkeypatch):
    """No Windows sem Modo Desenvolvedor o os.symlink falha; a conta não pode sobrar pela metade
    (marcador publicado, seletor mostrando conta quebrada). O rollback remove a pasta inteira."""
    def quebra(src, dst, **kw):
        raise OSError(1, "permission denied")

    monkeypatch.setattr(os, "symlink", quebra)
    with pytest.raises(contas.ContaError):
        contas.criar("conta2")
    assert not (casa / ".claude-conta2").exists()


def test_semear_com_claude_json_truncado_estoura(casa):
    """Arquivo truncado por escrita concorrente não vira conta "criada" sem MCP nem permissões,
    com o problema escondido — e a pasta parcial é desfeita."""
    (casa / ".claude.json").write_text("{tru", encoding="utf-8")
    with pytest.raises(contas.ContaError) as e:
        contas.criar("conta2")
    assert e.value.status == 500
    assert not (casa / ".claude-conta2").exists()


def test_semear_com_claude_json_nao_objeto_estoura(casa):
    (casa / ".claude.json").write_text('"sou uma string"', encoding="utf-8")
    with pytest.raises(contas.ContaError) as e:
        contas.criar("conta2")
    assert e.value.status == 500


def test_semear_com_erro_de_leitura_estoura(casa, monkeypatch):
    def quebra(*a, **kw):
        raise OSError(13, "permission denied")

    monkeypatch.setattr(pathlib.Path, "read_text", quebra)
    with pytest.raises(contas.ContaError) as e:
        contas.criar("conta2")
    assert e.value.status == 500


def test_memoria_local_vai_pra_gaveta_e_volta_a_ser_atalho(casa):
    """Memória local de verdade dentro da conta diverge do compartilhado calada — mesma regra do
    resto do ambiente: vai pra gaveta, o atalho é refeito e o aviso sai no reconciliar."""
    p = contas.criar("conta2")
    memo = p / "projects" / "-tmp-x" / "memory"
    memo.unlink()
    memo.mkdir()
    (memo / "MEMORY.md").write_text("versao so minha", encoding="utf-8")
    avisos = contas.reconciliar("conta2")
    assert memo.is_symlink()
    assert (memo / "MEMORY.md").read_text(encoding="utf-8") == "m"
    assert (p / ".drift" / "memory.1" / "MEMORY.md").read_text(encoding="utf-8") == "versao so minha"
    assert any("memory" in a for a in avisos)


def test_memoria_de_projeto_removido_e_podada(casa):
    """Projeto sumiu do compartilhado: o symlink de memory que ficou apontando pro nada é link
    morto, não dado — sai sem gaveta, na próxima reconciliação."""
    p = contas.criar("conta2")
    contas.reconciliar("conta2", projeto="-home-jefferson-saiu")
    memo = p / "projects" / "-home-jefferson-saiu" / "memory"
    assert memo.is_symlink()
    import shutil
    shutil.rmtree(casa / ".claude" / "projects" / "-home-jefferson-saiu")
    assert contas.reconciliar("conta2") == []
    assert not memo.exists()
    assert not memo.is_symlink()


def test_temporario_hangar_novo_do_usuario_nao_e_apagado(casa):
    """O temporário da troca atômica tinha nome fixo; um arquivo legítimo com esse nome era
    apagado na religação. Com nome único por chamada, o arquivo do usuário sobrevive."""
    p = contas.criar("conta2")
    (p / "skills.hangar-novo").write_text("dado meu", encoding="utf-8")
    (p / "skills").unlink()
    (p / "skills").mkdir()
    contas.reconciliar("conta2")
    assert (p / "skills.hangar-novo").read_text(encoding="utf-8") == "dado meu"
    assert (p / "skills").is_symlink()


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
    """O `$` casa antes do \n final; o fullmatch não. Sem isto, POST {"nome": "conta2\n"}
    criaria uma pasta com quebra de linha no nome."""
    with pytest.raises(contas.ContaError):
        contas.criar("conta2\n")


def test_nome_com_barra_e_recusado(casa):
    with pytest.raises(contas.ContaError):
        contas.criar("a/b")


def test_nome_pontinhos_e_recusado(casa):
    with pytest.raises(contas.ContaError):
        contas.criar("..")


def test_nome_muito_longo_e_recusado(casa):
    with pytest.raises(contas.ContaError):
        contas.criar("a" * 33)


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
    chata de diagnosticar. Prova real: subprocesso com `-S` (sem site-packages), só o backend no
    sys.path. O AST fica como diagnóstico pra apontar o import culpado se o subprocesso falhar."""
    raiz = str(pathlib.Path(contas.__file__).resolve().parent.parent)
    r = subprocess.run(
        [sys.executable, "-S", "-c",
         f"import sys; sys.path.insert(0, {raiz!r}); import app.contas"],
        capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
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
