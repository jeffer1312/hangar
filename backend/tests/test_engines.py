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
    env = {**os.environ, "CP_ENGINES_FILE": str(cfg or eng.caminho())}
    return subprocess.run([sys.executable, str(CLI), *args],
                          capture_output=True, text=True, env=env)


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


def test_modulo_e_stdlib_pura():
    # O cp-engine importa este módulo com o python3 do SISTEMA (sem venv). Um import de app.config
    # puxaria pydantic e quebraria o terminal, deixando só o app funcionando — falha assimétrica,
    # chata de diagnosticar. Sentinela, não prova: barra os culpados conhecidos.
    fonte = pathlib.Path(eng.__file__).read_text(encoding="utf-8")
    arvore = ast.parse(fonte)
    importados = {
        n.module.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.ImportFrom) and n.module
    } | {
        a.name.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.Import) for a in n.names
    }
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
