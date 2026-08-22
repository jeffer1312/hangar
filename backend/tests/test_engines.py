"""Motores de modelo: um JSON por baixo, env derivado por cima.

O que esta suíte trava: valor com quebra de linha não entra (ele vira `export` no shell), os 6 nomes
de modelo andam JUNTOS (faltar um quebra subagent, calado), a janela usa MAX_CONTEXT_TOKENS (a outra
var é inerte — medido nos dois provedores), base_url insegura não entra (a key sairia em claro) e
motor desconhecido estoura em vez de devolver env vazio (a sessão subiria na conta Anthropic achando
que é o motor pedido).
"""
import ast
import json
import logging
import os
import pathlib
import subprocess
import sys

import pytest

from app import engines as eng

CLI = pathlib.Path(__file__).resolve().parents[2] / "scripts" / "cp-engine"


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(eng, "caminho", lambda: tmp_path / "engines.json")
    yield


def _kimi() -> dict:
    return {
        "label": "Kimi Code · K3",
        "base_url": "https://api.kimi.com/coding",
        "api_key": "sk-kimi-abcdefgh1234",
        "model": "k3",
        "context_window": 262144,
        "vision": True,
    }


def test_env_repete_o_modelo_nas_seis_vars():
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    seis = [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ]
    assert [env[k] for k in seis] == ["k3"] * 6


def test_env_usa_auth_token_e_nunca_api_key():
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    assert env["ANTHROPIC_AUTH_TOKEN"] == "sk-kimi-abcdefgh1234"
    assert "ANTHROPIC_API_KEY" not in env


def test_env_marca_o_motor_para_o_proc_reconhecer():
    eng.salvar("kimi", _kimi())
    assert eng.env_de("kimi")["CP_ENGINE"] == "kimi"


def test_janela_usa_max_context_tokens_e_nao_a_var_inerte():
    # Medido nos dois provedores: AUTO_COMPACT_WINDOW não move a janela (o /context seguia em 200k) e
    # MAX_CONTEXT_TOKENS move. Var de doc de terceiro sem efeito medido não entra.
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    assert env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] == "262144"
    assert "CLAUDE_CODE_AUTO_COMPACT_WINDOW" not in env


def test_sem_janela_nao_inventa_a_var():
    d = _kimi()
    del d["context_window"]
    eng.salvar("kimi", d)
    assert "CLAUDE_CODE_MAX_CONTEXT_TOKENS" not in eng.env_de("kimi")


def test_subagent_model_sobrescreve_a_var_dos_subagentes():
    d = _kimi()
    d["subagent_model"] = "k3-fast"
    eng.salvar("kimi", d)
    assert eng.env_de("kimi")["CLAUDE_CODE_SUBAGENT_MODEL"] == "k3-fast"


def test_sem_subagent_model_cai_no_modelo_principal():
    # Campo ausente = "mesmo que o principal", nunca uma env var vazia (subagent/background
    # falhariam sem mensagem clara — mesma razão dos outros 5 ANTHROPIC_DEFAULT_*_MODEL).
    eng.salvar("kimi", _kimi())
    assert eng.env_de("kimi")["CLAUDE_CODE_SUBAGENT_MODEL"] == "k3"


def test_motor_desconhecido_estoura_em_vez_de_env_vazio():
    with pytest.raises(KeyError):
        eng.env_de("nao-existe")


def test_valor_com_quebra_de_linha_e_recusado():
    # `cp-engine --env` imprime CHAVE=VALOR por linha e o shell dá export nisso: um \n na key
    # exportaria uma variável arbitrária (ex: PATH) no shell que vai rodar o claude.
    for campo, valor in (("api_key", "sk-x\nPATH=/tmp/evil"),
                         ("model", "k3\nHOME=/tmp"),
                         ("base_url", "https://a.b\nX=1"),
                         ("api_key", "sk-x\rRETURN"),
                         ("model", "k3\x00NULL"),
                         ("base_url", "https://a.b\r\nCRLF")):
        d = _kimi()
        d[campo] = valor
        with pytest.raises(ValueError, match="linha"):
            eng.salvar("x", d)


def test_http_publico_e_recusado():
    d = _kimi()
    d["base_url"] = "http://api.kimi.com/coding"
    with pytest.raises(ValueError, match="https"):
        eng.salvar("kimi", d)


def test_http_em_loopback_e_em_rede_privada_e_aceito():
    # É o caso do proxy tradutor local (LiteLLM) e de um gateway na LAN.
    for url in ("http://127.0.0.1:4000", "http://192.168.1.50:4000", "http://localhost:8080"):
        d = _kimi()
        d["base_url"] = url
        eng.salvar("proxy", d)
        assert eng.env_de("proxy")["ANTHROPIC_BASE_URL"] == url


def test_base_url_perde_a_barra_final():
    # O CC monta {base}/v1/messages; barra sobrando geraria //v1/messages.
    d = _kimi()
    d["base_url"] = "https://api.kimi.com/coding/"
    eng.salvar("kimi", d)
    assert eng.env_de("kimi")["ANTHROPIC_BASE_URL"] == "https://api.kimi.com/coding"


def test_campo_obrigatorio_faltando_estoura():
    for faltando in ("base_url", "api_key", "model"):
        d = _kimi()
        del d[faltando]
        with pytest.raises(ValueError, match=faltando):
            eng.salvar("x", d)


def test_campo_desconhecido_e_descartado():
    d = _kimi()
    d["rode_isto"] = "rm -rf /"
    eng.salvar("kimi", d)
    assert "rode_isto" not in eng.listar()["kimi"]


def test_nome_de_motor_e_sanitizado():
    for ruim in ("../fuga", "COM MAIUSCULA", "", "com espaco"):
        with pytest.raises(ValueError, match="nome"):
            eng.salvar(ruim, _kimi())


def test_remover_devolve_se_existia():
    eng.salvar("kimi", _kimi())
    assert eng.remover("kimi") is True
    assert eng.remover("kimi") is False


@pytest.mark.skipif(os.name != "posix",
                    reason="nao ha bit de modo no Windows (st_mode volta 0o666 e quem decide e a "
                           "ACL) — e o engines.json guarda CHAVE DE API, entao la ele fica sem a "
                           "protecao que este caso cobra; lacuna conhecida, nao teste ruim")
def test_arquivo_nasce_0600():
    eng.salvar("kimi", _kimi())
    assert (eng.caminho().stat().st_mode & 0o777) == 0o600


def test_arquivo_corrompido_nao_derruba_a_leitura():
    eng.caminho().write_text("{lixo", encoding="utf-8")
    assert eng.listar() == {}


def test_gravar_nao_perde_o_motor_do_vizinho():
    eng.salvar("kimi", _kimi())
    d = _kimi()
    d["model"] = "codex/gpt-5.6-sol"
    d["base_url"] = "https://ai.omniwise.com.br"
    eng.salvar("omniroute", d)
    assert set(eng.listar()) == {"kimi", "omniroute"}


def test_env_de_rejeita_json_envenenado_com_newline():
    # engines.json pode ser hand-editado ou corrompido; env_de não deve emitir valores com
    # caractere proibido. Contratos de shell-safety são enforçados na leitura, não só na escrita.
    d = _kimi()
    eng.salvar("kimi", d)
    # Simula hand-editing: escreve JSON com newline na api_key, bypassing salvar
    payload = {"kimi": d}
    payload["kimi"]["api_key"] = "sk-x\nPATH=/tmp/evil"
    eng.caminho().write_text(json.dumps(payload), encoding="utf-8")
    # env_de deve rejeitar com ValueError, não retornar env perigosa
    with pytest.raises(ValueError, match="api_key"):
        eng.env_de("kimi")


def test_env_de_rejeita_json_envenenado_em_multiplos_campos():
    # Cobre outros campos e caracteres: carriage return, null byte.
    d = _kimi()
    # Escreve JSON poisonado diretamente para test_env_de_rejeita_json_envenenado_mostra_campo_e_mensagem
    eng.salvar("kimi", d)
    payload = {"kimi": d}
    # Test 1: model com null byte
    payload["kimi"]["model"] = "k3\x00EVIL"
    eng.caminho().write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="model.*proibido"):
        eng.env_de("kimi")
    # Test 2: base_url com carriage return
    payload["kimi"] = _kimi()
    payload["kimi"]["base_url"] = "https://a.b\rINJECT"
    eng.caminho().write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="base_url.*proibido"):
        eng.env_de("kimi")


def test_env_de_rejeita_context_window_envenenado_hand_editado():
    # context_window é int por contrato (salvar() barra qualquer outra coisa), mas o arquivo é
    # hand-editável: escrever uma string com \n faz `cp-engine --env` emitir uma linha extra (medido
    # ao vivo). str(int(...)) tem que estourar em vez de deixar passar.
    d = _kimi()
    eng.salvar("kimi", d)
    payload = {"kimi": {**d, "context_window": "1\nPATH=/tmp/evil"}}
    eng.caminho().write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError):
        eng.env_de("kimi")


# ---------------------------------------------------------------------------
# Fix wave (pré-push), item 1: arquivo corrompido não pode virar "nenhum motor configurado" calado.
# O bug real: hand-edit typo quebra o JSON, listar() some com kimi+omniroute, sheet mostra "nenhum
# motor ainda", usuário re-adiciona "kimi" achando que é a primeira vez, e salvar() (que chamava
# listar() -> {}) sobrescrevia o disco só com o kimi novo — omniroute e a key dele, perdidos pra
# sempre, sem log e sem aviso.
# ---------------------------------------------------------------------------

def test_arquivo_corrompido_loga_o_aviso(caplog):
    eng.caminho().write_text("{lixo", encoding="utf-8")
    with caplog.at_level(logging.WARNING, logger="app.engines"):
        assert eng.listar() == {}
    assert any("engines.json" in r.getMessage() for r in caplog.records)


def test_arquivo_ausente_fica_quieto(caplog):
    # Ausente é o estado NORMAL (ninguém configurou nada ainda) — não pode logar warning a cada
    # boot/tick do SSE só porque o usuário nunca cadastrou um motor.
    with caplog.at_level(logging.WARNING, logger="app.engines"):
        assert eng.listar() == {}
    assert caplog.records == []


def test_arquivo_corrompido_reporta_true_so_quando_ilegivel():
    assert eng.arquivo_corrompido() is False
    eng.caminho().write_text("{lixo", encoding="utf-8")
    assert eng.arquivo_corrompido() is True


def test_salvar_recusa_gravar_por_cima_de_arquivo_corrompido():
    bruto = "{lixo"
    eng.caminho().write_text(bruto, encoding="utf-8")
    with pytest.raises(ValueError, match="corrompido"):
        eng.salvar("kimi", _kimi())
    # O ponto inteiro do fix: perder a GRAVAÇÃO é recuperável (o usuário tenta de novo depois de
    # corrigir o arquivo); perder o ARQUIVO (sobrescrito com {"kimi": ...} só) não é. Bytes no disco
    # têm que continuar exatamente os mesmos de antes da chamada.
    assert eng.caminho().read_text(encoding="utf-8") == bruto


def test_remover_recusa_gravar_por_cima_de_arquivo_corrompido():
    bruto = "{lixo"
    eng.caminho().write_text(bruto, encoding="utf-8")
    with pytest.raises(ValueError, match="corrompido"):
        eng.remover("kimi")
    assert eng.caminho().read_text(encoding="utf-8") == bruto


def test_listar_pula_nome_invalido_sem_derrubar_os_outros():
    # registry.py interpola o nome num `$SHELL -c` (cp-engine --exec {nome} -- ...). salvar() já
    # barra nome fora do padrão, mas o JSON é hand-editável; um registro corrupto não pode tirar os
    # outros motores do ar.
    eng.salvar("kimi", _kimi())
    bruto = json.loads(eng.caminho().read_text(encoding="utf-8"))
    bruto["x; touch /tmp/PWNED"] = _kimi()
    eng.caminho().write_text(json.dumps(bruto), encoding="utf-8")
    assert set(eng.listar()) == {"kimi"}


def _cli(*args, cfg=None):
    # PYTHONIOENCODING + encoding no decode: os dois lados FIXADOS, senao o teste depende do locale
    # de quem roda. No Windows o filho escreveria stderr em cp1252 (ou em utf-8, se quem chamou por
    # acaso tivesse a variavel no ambiente) e o pai decodificaria com o locale — e um assert por
    # substring com acento passava a depender do ambiente, nao do codigo. Foi assim que
    # "opção desconhecida" virou "opÃ§Ã£o desconhecida" aqui. No Linux nao muda nada: ja era utf-8
    # dos dois lados.
    env = {**os.environ, "CP_ENGINES_FILE": str(cfg or eng.caminho()),
           "PYTHONIOENCODING": "utf-8"}
    return subprocess.run([sys.executable, str(CLI), *args],
                          capture_output=True, text=True, env=env,
                          encoding="utf-8", errors="replace")


def test_cli_env_imprime_chave_igual_valor():
    # É este formato que o claude-engine consome. O monkeypatch de eng.caminho não vale no
    # subprocess, então o filho recebe CP_ENGINES_FILE — que é o que caminho() lê de verdade.
    eng.salvar("kimi", _kimi())
    r = _cli("--env", "kimi")
    assert r.returncode == 0, r.stderr
    linhas = dict(l.split("=", 1) for l in r.stdout.strip().splitlines())
    assert linhas["ANTHROPIC_MODEL"] == "k3"
    assert linhas["CP_ENGINE"] == "kimi"
    assert "ANTHROPIC_API_KEY" not in linhas


def test_cli_env_de_motor_inexistente_sai_com_erro():
    # Sair 0 com stdout vazio abriria a sessão na conta Anthropic achando que é o motor pedido.
    r = _cli("--env", "fantasma")
    assert r.returncode != 0
    assert "fantasma" in r.stderr


def test_cli_list_uma_linha_por_motor():
    eng.salvar("kimi", _kimi())
    r = _cli("--list")
    assert r.returncode == 0
    assert r.stdout.strip().split("\t")[:1] == ["kimi"]


def test_cli_exec_aplica_o_env_no_processo_filho():
    # O CORAÇÃO da feature: o env entra no processo que substitui o cp-engine, não em linha de comando.
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--", sys.executable, "-c",
             "import os;print(os.environ['ANTHROPIC_MODEL'], os.environ['CP_ENGINE'])")
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "k3 kimi"


@pytest.mark.skipif(os.name != "posix",
                    reason="le /proc/<pid>/cmdline pra provar que a key nao vaza; no Windows nao ha /proc, e a propriedade equivalente (cmdline legivel por outro usuario) tem outro mecanismo")
def test_cli_exec_nao_deixa_o_segredo_no_cmdline():
    # /proc/<pid>/cmdline é legível por qualquer usuário da máquina. Depois do execvpe o cmdline é o
    # do comando alvo; a key só existe no environ.
    #
    # A key é lida de os.environ DENTRO do processo filho (não escrita como literal no código -c):
    # embutir "sk-kimi" no source o transformaria em argv do próprio comando -c, e o teste passaria
    # sempre (falso negativo) por casar consigo mesmo, não por checar o vazamento de verdade.
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--", sys.executable, "-c",
             "import pathlib,os;"
             "segredo=os.environ['ANTHROPIC_AUTH_TOKEN'].encode();"
             "print(pathlib.Path(f'/proc/{os.getpid()}/cmdline').read_bytes().count(segredo))")
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "0"


def test_cli_exec_de_motor_inexistente_nao_roda_o_comando():
    r = _cli("--exec", "fantasma", "--", sys.executable, "-c", "print('RODOU')")
    assert r.returncode != 0
    assert "RODOU" not in r.stdout
    assert "fantasma" in r.stderr


def test_cli_env_de_motor_envenenado_sai_com_erro_e_nome_do_campo():
    # engines.json é hand-editável; env_de agora levanta ValueError (não só KeyError) pra valor com
    # \n/\r/\0. cp-engine tem que morrer igual nos dois casos — traceback aqui vazaria pro usuário
    # errado (o wrapper de shell) em vez de uma mensagem clara.
    d = _kimi()
    eng.salvar("kimi", d)
    payload = {"kimi": {**d, "api_key": "sk-x\nPATH=/tmp/evil"}}
    eng.caminho().write_text(json.dumps(payload), encoding="utf-8")
    r = _cli("--env", "kimi")
    assert r.returncode != 0
    assert "api_key" in r.stderr
    assert "Traceback" not in r.stderr


def test_cli_exec_de_motor_envenenado_nao_roda_o_comando():
    d = _kimi()
    eng.salvar("kimi", d)
    payload = {"kimi": {**d, "api_key": "sk-x\nPATH=/tmp/evil"}}
    eng.caminho().write_text(json.dumps(payload), encoding="utf-8")
    r = _cli("--exec", "kimi", "--", sys.executable, "-c", "print('RODOU')")
    assert r.returncode != 0
    assert "RODOU" not in r.stdout
    assert "api_key" in r.stderr


def _importados_de(caminho: pathlib.Path) -> set[str]:
    arvore = ast.parse(caminho.read_text(encoding="utf-8"))
    return {
        n.module.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.ImportFrom) and n.module
    } | {
        a.name.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.Import) for a in n.names
    }


def _nomes_importados_de_app(caminho: pathlib.Path) -> set[str]:
    """O que exatamente o modulo tira de `app` (`from app import x, y` -> {x, y})."""
    arvore = ast.parse(caminho.read_text(encoding="utf-8"))
    return {a.name for n in ast.walk(arvore)
            if isinstance(n, ast.ImportFrom) and n.module == "app" for a in n.names}


# Unico modulo de `app` que o engines.py pode importar. Entrou porque o `os.replace` cru falha no
# Windows quando outro processo tem o destino aberto (e o cp-engine LE o engines.json — e
# exatamente esse leitor), e duplicar a retentativa aqui seria uma segunda verdade. Qualquer nome
# novo nesta lista tem que ser stdlib puro, e o caso abaixo cobra isso.
_APP_PERMITIDO = {"atomico"}


def test_modulo_e_stdlib_pura():
    # O cp-engine importa este módulo com o python3 do SISTEMA (sem venv). Um import de app.config
    # puxaria pydantic e quebraria o terminal, deixando só o app funcionando — falha assimétrica,
    # chata de diagnosticar. Sentinela, não prova: barra os culpados conhecidos.
    importados = _importados_de(pathlib.Path(eng.__file__))
    assert not (importados & {"pydantic", "fastapi", "httpx", "httpx2"})
    # `app` deixou de ser proibido em bloco e passou a ser uma allowlist: barrar o pacote inteiro
    # impediria reusar codigo que É stdlib puro, e liberar o pacote inteiro devolveria o furo.
    assert _nomes_importados_de_app(pathlib.Path(eng.__file__)) <= _APP_PERMITIDO


@pytest.mark.parametrize("nome", sorted(_APP_PERMITIDO))
def test_o_que_o_engines_importa_de_app_tambem_e_stdlib_puro(nome):
    """A pureza tem que valer TRANSITIVAMENTE, senao a allowlist vira a porta que ela fechou."""
    modulo = pathlib.Path(eng.__file__).parent / f"{nome}.py"
    assert modulo.is_file(), f"app/{nome}.py nao existe"
    importados = _importados_de(modulo)
    assert not (importados & {"app", "pydantic", "fastapi", "httpx", "httpx2"})


def test_bundled_skills_desligadas_por_padrao_no_motor():
    """Skill empacotada injeta a árvore inteira num turno — medido: a `claude-api` (64 arquivos, sem
    SKILL.md na raiz) colou 847.630 chars / 206.553 tokens numa sessão gpt-5.6-sol, levando a janela
    de 12% a 67% e matando a sessão no turno seguinte. Motor não tem a poda server-side da Anthropic,
    então o default é desligado; quem quiser liga explicitamente."""
    eng.salvar("kimi", _kimi())
    assert eng.env_de("kimi")["CLAUDE_CODE_DISABLE_BUNDLED_SKILLS"] == "1"


def test_bundled_skills_ligadas_nao_exportam_a_var():
    dados = _kimi() | {"bundled_skills": True}
    eng.salvar("kimi", dados)
    assert "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS" not in eng.env_de("kimi")


def test_betas_experimentais_desligados_por_padrao_no_motor():
    """`400 Extra inputs are not permitted` citando context_management é o sintoma documentado de um
    upstream de terceiro recusando os campos beta que o CC manda sozinho."""
    eng.salvar("kimi", _kimi())
    assert eng.env_de("kimi")["CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS"] == "1"


def test_prompt_caching_fica_ligado_sem_pedido_explicito():
    """Default LIGADO: cache é economia e já degrada calado em gateway. Só sai com false explícito."""
    eng.salvar("kimi", _kimi())
    assert "DISABLE_PROMPT_CACHING" not in eng.env_de("kimi")
    eng.salvar("kimi", _kimi() | {"prompt_caching": False})
    assert eng.env_de("kimi")["DISABLE_PROMPT_CACHING"] == "1"


def test_adaptive_thinking_fica_ligado_sem_pedido_explicito():
    """Desligar thinking REBAIXA o modelo em alguns provedores (doc da Kimi: K3 -> K2.6), então isto
    nunca pode sair de um default — só de escolha explícita."""
    eng.salvar("kimi", _kimi())
    assert "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING" not in eng.env_de("kimi")
    eng.salvar("kimi", _kimi() | {"adaptive_thinking": False})
    assert eng.env_de("kimi")["CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"] == "1"


def test_opt_ins_so_saem_quando_marcados():
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    assert "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY" not in env
    assert "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING" not in env
    eng.salvar("kimi", _kimi() | {"gateway_model_discovery": True,
                                  "fine_grained_tool_streaming": True})
    env = eng.env_de("kimi")
    assert env["CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"] == "1"
    assert env["CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING"] == "1"


def test_janelas_numericas_so_saem_com_valor():
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    assert "CLAUDE_CODE_AUTO_COMPACT_WINDOW" not in env
    assert "CLAUDE_CODE_MAX_OUTPUT_TOKENS" not in env
    eng.salvar("kimi", _kimi() | {"auto_compact_window": 150000, "max_output_tokens": 16000})
    env = eng.env_de("kimi")
    assert env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"] == "150000"
    assert env["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] == "16000"


def test_tool_search_nao_e_efetivo_sem_os_betas():
    """Dependência documentada: DISABLE_EXPERIMENTAL_BETAS mantém tool search off e ENABLE_TOOL_SEARCH
    não sobrepõe. O env reflete o pedido; quem avisa o usuário é a UI (toggle desabilitado)."""
    eng.salvar("kimi", _kimi() | {"tool_search": True})
    env = eng.env_de("kimi")
    assert "ENABLE_TOOL_SEARCH" not in env                       # tool_search=true não emite a var
    assert env["CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS"] == "1"  # ...mas os betas seguem off


@pytest.mark.parametrize("campo", ["context_window", "auto_compact_window", "max_output_tokens"])
def test_env_de_rejeita_inteiro_negativo_hand_editado(campo, tmp_path, monkeypatch):
    """engines.json é hand-editável e `_normalizar` só roda no SAVE. Um negativo no disco virava
    `CLAUDE_CODE_MAX_CONTEXT_TOKENS=-1000` — o Claude Code não valida, então a sessão subia com uma
    janela absurda e a falha aparecia longe da causa. Recusa aqui, como já se faz com string
    envenenada."""
    eng.salvar("kimi", _kimi())
    bruto = json.loads((tmp_path / "engines.json").read_text())
    bruto["kimi"][campo] = -1000
    (tmp_path / "engines.json").write_text(json.dumps(bruto))
    with pytest.raises(ValueError, match="maior que zero"):
        eng.env_de("kimi")


@pytest.mark.parametrize("campo", ["auto_compact_window", "max_output_tokens"])
def test_salvar_rejeita_janela_nao_positiva(campo):
    with pytest.raises(ValueError, match="maior que zero"):
        eng.salvar("kimi", _kimi() | {campo: 0})
    with pytest.raises(ValueError, match="esperado número"):
        eng.salvar("kimi", _kimi() | {campo: "abc"})


@pytest.mark.parametrize("campo", ["bundled_skills", "tool_search", "experimental_betas",
                                   "prompt_caching", "adaptive_thinking",
                                   "gateway_model_discovery", "fine_grained_tool_streaming"])
def test_env_de_rejeita_booleano_envenenado_hand_editado(campo, tmp_path, monkeypatch):
    """`1 is not True` é verdadeiro em Python. Um `"tool_search": 1` hand-editado — natural pra quem
    viu a convenção "1"/"0" das env vars ao lado — caía no ramo DESLIGADO, o oposto do pedido, calado.
    Recusa o arquivo em vez de adivinhar."""
    eng.salvar("kimi", _kimi())
    bruto = json.loads((tmp_path / "engines.json").read_text())
    bruto["kimi"][campo] = 1
    (tmp_path / "engines.json").write_text(json.dumps(bruto))
    with pytest.raises(ValueError, match="esperado true/false"):
        eng.env_de("kimi")


def test_avisa_no_log_quando_tool_search_fica_inerte(caplog):
    """A UI desabilita o toggle, mas não é o único cliente da API. Sem o log, `tool_search: true`
    aparece ligado no GET e ninguém descobre que está inerte."""
    eng.salvar("kimi", _kimi() | {"tool_search": True})
    with caplog.at_level(logging.WARNING):
        eng.env_de("kimi")
    assert any("não tem efeito" in r.getMessage() for r in caplog.records), caplog.text


def test_sem_aviso_quando_a_combinacao_e_coerente(caplog):
    eng.salvar("kimi", _kimi() | {"tool_search": True, "experimental_betas": True})
    with caplog.at_level(logging.WARNING):
        eng.env_de("kimi")
    assert "não tem efeito" not in caplog.text


# ── Task 3: modelo e janela escolhidos na abertura entram no env ───────────────────────────────


def test_env_com_modelo_troca_as_cinco_chaves_de_modelo():
    """`--model` sozinho ganha só de ANTHROPIC_MODEL; os quatro aliases continuariam no modelo do
    motor, e `/model opus` dentro da sessão voltaria pra ele."""
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi", modelo="k3-256k")
    for chave in ("ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL",
                  "ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_DEFAULT_FABLE_MODEL"):
        assert env[chave] == "k3-256k"


def test_subagente_segue_o_modelo_escolhido_quando_o_motor_nao_fixou_um():
    eng.salvar("kimi", _kimi())            # _kimi() não define subagent_model
    assert eng.env_de("kimi", modelo="k3-256k")["CLAUDE_CODE_SUBAGENT_MODEL"] == "k3-256k"


def test_subagent_model_configurado_ganha_do_modelo_escolhido():
    """Decisão explícita: quem escreveu subagent_model no motor fez escolha DE CUSTO (subagente faz
    busca mecânica; modelo barato ali é dinheiro). Trocar o modelo principal não desfaz isso."""
    cfg = {**_kimi(), "subagent_model": "kimi-for-coding"}
    eng.salvar("kimi", cfg)
    assert eng.env_de("kimi", modelo="k3-256k")["CLAUDE_CODE_SUBAGENT_MODEL"] == "kimi-for-coding"


def test_env_com_janela_do_modelo_escolhido():
    """Motor de 1M mais modelo de 262k faria a sessão compactar só em 1M e estourar no provedor — o
    inverso exato do bug que MAX_CONTEXT_TOKENS existe pra corrigir."""
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi", modelo="k3-256k", context_window=262144)
    assert env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] == "262144"


def test_modelo_escolhido_sem_janela_conhecida_omite_a_variavel():
    """Provedor que não reporta context_length (opencode zen, medido). Exportar a janela do MOTOR
    com outro modelo é o bug de volta; omitir deixa o CLI usar o default dele."""
    eng.salvar("kimi", _kimi())            # _kimi() tem context_window de 262k
    env = eng.env_de("kimi", modelo="k3-256k", context_window=None)
    assert "CLAUDE_CODE_MAX_CONTEXT_TOKENS" not in env


def test_env_sem_modelo_e_identico_ao_de_hoje():
    eng.salvar("kimi", _kimi())
    assert eng.env_de("kimi") == eng.env_de("kimi", modelo=None, context_window=None)


def test_modelo_com_caractere_proibido_e_recusado():
    eng.salvar("kimi", _kimi())
    with pytest.raises(ValueError):
        eng.env_de("kimi", modelo="k3-256k\nrm -rf /")


def test_motor_sem_janela_configurada_usa_a_do_modelo_escolhido():
    """context_window do motor é OPCIONAL. Sem ele, a janela do modelo escolhido é a única que
    existe — descartá-la deixa a sessão compactando em ~167k com um modelo de 262k."""
    cfg = {k: v for k, v in _kimi().items() if k != "context_window"}
    eng.salvar("kimi", cfg)
    assert "CLAUDE_CODE_MAX_CONTEXT_TOKENS" not in eng.env_de("kimi")          # segue como hoje
    env = eng.env_de("kimi", modelo="k3-256k", context_window=262144)
    assert env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] == "262144"


def test_cli_exec_janela_do_modelo_em_motor_sem_janela_configurada():
    """Ponta a ponta: a flag --context não pode ser aceita e descartada."""
    eng.salvar("kimi", {k: v for k, v in _kimi().items() if k != "context_window"})
    r = _cli("--exec", "kimi", "--model", "k3-256k", "--context", "262144", "--",
             sys.executable, "-c",
             "import os;print(os.environ.get('CLAUDE_CODE_MAX_CONTEXT_TOKENS'))")
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "262144"


def test_cli_exec_com_modelo_e_janela_aplica_env_no_filho():
    # O parse novo do cp-engine: `--model` e `--context` viram parte do env, não do cmdline.
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--model", "k3-256k", "--context", "262144", "--",
             sys.executable, "-c",
             "import os;print(os.environ['ANTHROPIC_MODEL'], "
             "os.environ['CLAUDE_CODE_MAX_CONTEXT_TOKENS'])")
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "k3-256k 262144"


def test_cli_exec_sem_flags_novas_continua_igual():
    # A forma antiga (`cp-engine --exec <motor> -- <cmd>`) não pode regredir: é o que o backend
    # usava até a Task 3 e continua sendo o caminho de sessão sem escolha.
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--", sys.executable, "-c",
             "import os;print(os.environ['ANTHROPIC_MODEL'])")
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == "k3"


def test_cli_exec_model_sem_valor_erro_limpo():
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--model", "--", sys.executable, "-c", "print('RODOU')")
    assert r.returncode == 2
    assert "--model precisa de um valor" in r.stderr
    assert "RODOU" not in r.stdout
    assert "Traceback" not in r.stderr


def test_cli_exec_context_nao_numerico_erro_limpo():
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--context", "abc", "--", sys.executable, "-c", "print('RODOU')")
    assert r.returncode == 2
    assert "--context precisa de um número" in r.stderr
    assert "RODOU" not in r.stdout
    assert "Traceback" not in r.stderr


def test_cli_exec_opcao_desconhecida_erro_limpo():
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--bogus", "x", "--", sys.executable, "-c", "print('RODOU')")
    assert r.returncode == 2
    assert "opção desconhecida" in r.stderr
    assert "Traceback" not in r.stderr


def test_cli_exec_sem_comando_depois_do_separador_erro_limpo():
    # `cp-engine --exec kimi --model x --` passaria do parse e `cmd[0]` estouraria IndexError —
    # traceback cru, o oposto do que este parse existe pra fazer.
    eng.salvar("kimi", _kimi())
    r = _cli("--exec", "kimi", "--model", "k3-256k", "--")
    assert r.returncode == 2
    assert "falta o comando depois de --" in r.stderr
    assert "Traceback" not in r.stderr
