"""Camada de config editável em runtime: override em JSON por cima do env.

O que esta suíte trava: segredo nunca volta inteiro, campo desconhecido não vira setting, e
gravar é atômico (arquivo pela metade viraria "sem config nenhuma", calado).
"""
import json

import pytest

from app import runtime_config as rc


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    yield


def test_sem_arquivo_vale_o_env(monkeypatch):
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    assert rc.get("upload_retention_days") == 30
    assert rc.estado()["upload_retention_days"]["origem"] == "env"


def test_override_vence_o_env(monkeypatch):
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    rc.aplicar({"upload_retention_days": 7})
    assert rc.get("upload_retention_days") == 7
    assert rc.estado()["upload_retention_days"]["origem"] == "app"


def test_campo_desconhecido_e_ignorado():
    rc.aplicar({"auth_token": "roubado", "port": 1})
    salvo = json.loads((rc._caminho()).read_text(encoding="utf-8"))
    assert salvo == {}          # o cliente não inventa setting


def test_preferencia_statusline_ligada_por_padrao_e_persistida():
    assert rc.get("claude_statusline_update") is True
    rc.aplicar({"claude_statusline_update": False})
    assert rc.estado()["claude_statusline_update"]["valor"] is False
    assert json.loads(rc._caminho().read_text())["claude_statusline_update"] is False
    with pytest.raises(ValueError):
        rc.aplicar({"claude_statusline_update": "false"})


def test_voz_codex_beta_nasce_desligada_e_exige_booleano():
    assert rc.get("codex_voice_beta") is False
    rc.aplicar({"codex_voice_beta": True})
    assert rc.estado()["codex_voice_beta"]["valor"] is True
    with pytest.raises(ValueError):
        rc.aplicar({"codex_voice_beta": "true"})
    rc.aplicar({}, remover={"codex_voice_beta"})
    assert rc.override("codex_voice_beta") == (False, None)


def test_segredo_volta_mascarado_nunca_inteiro():
    rc.aplicar({"groq_api_key": "gsk_abcdefghijklmnop"})
    est = rc.estado()["groq_api_key"]
    assert est["definido"] is True
    assert est["valor"] == "gsk_••••••••mnop"
    assert "abcdefghij" not in est["valor"]
    # O valor real continua acessível pro backend usar.
    assert rc.get("groq_api_key") == "gsk_abcdefghijklmnop"


def test_devolver_a_mascara_exata_nao_apaga_a_chave():
    """Reenviar EXATAMENTE o que o GET devolveu preserva o segredo.

    A versão anterior deste teste usava "••••••••" — uma forma que `mascarar()` nunca produz pra
    chave real (>8 chars vira mista: gsk_••••••••1234). O teste passava e o caminho de verdade
    estava quebrado: encostar no campo sobrescrevia a chave pelo texto mascarado. Aqui o valor
    reenviado sai de `estado()`, que é a fonte que o cliente enxerga.
    """
    real = "gsk_abcdefghijklmnop"
    rc.aplicar({"groq_api_key": real})
    mascara = rc.estado()["groq_api_key"]["valor"]
    assert mascara != real and "•" in mascara
    rc.aplicar({"groq_api_key": mascara})          # o que o front reenviaria
    assert rc.get("groq_api_key") == real
    rc.aplicar({"groq_api_key": "  " + mascara + " "})   # com espaço do teclado
    assert rc.get("groq_api_key") == real


def test_chave_nova_de_verdade_substitui():
    rc.aplicar({"groq_api_key": "gsk_primeira_chave_aqui"})
    rc.aplicar({"groq_api_key": "gsk_segunda_chave_nova"})
    assert rc.get("groq_api_key") == "gsk_segunda_chave_nova"


def test_dois_patch_concorrentes_nao_perdem_mudanca():
    """Sem lock, os dois liam o mesmo estado e o último a gravar apagava o outro."""
    import threading

    rc.aplicar({"editor": "code"})
    erros = []

    def grava(campo, valor):
        try:
            rc.aplicar({campo: valor})
        except Exception as e:  # pragma: no cover
            erros.append(e)

    ts = [threading.Thread(target=grava, args=("upload_retention_days", 11)),
          threading.Thread(target=grava, args=("stall_seconds", 222))]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert not erros
    assert rc.get("upload_retention_days") == 11
    assert rc.get("stall_seconds") == 222      # as DUAS sobrevivem


def test_tipos_invalidos_sao_recusados():
    with pytest.raises(ValueError):
        rc.aplicar({"upload_retention_days": "trinta"})
    with pytest.raises(ValueError):
        rc.aplicar({"upload_retention_days": -1})
    with pytest.raises(ValueError):
        rc.aplicar({"automations": "sim"})


def test_arquivo_corrompido_nao_derruba(monkeypatch):
    # encoding explícito: sem ele o `ã` virava cp1252 no Windows, e o que o caso exercitava lá era
    # byte inválido, não JSON inválido — dois defeitos diferentes, um deles por acidente.
    rc._caminho().write_text("{ isso não é json", encoding="utf-8")
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    assert rc.get("upload_retention_days") == 30      # cai pro env, sem exceção


def test_arquivo_nao_utf8_nao_derruba(monkeypatch):
    """A outra forma de corrompido, e a que o except não pegava: bytes que não são utf-8.

    Acontece de verdade — arquivo salvo à mão num editor cp1252, ou escrita cortada no meio de um
    caractere multibyte. `_carregar` lê como utf-8, então isso levanta UnicodeDecodeError, que não
    é json.JSONDecodeError e escapava por cima de `get()` — num caminho quente
    (`automations_enabled`, `resolve_scan_roots`).
    """
    rc._caminho().write_bytes(b'{"automations": "n\xe3o"}')   # 0xe3 = 'ã' em cp1252, inválido em utf-8
    monkeypatch.setattr(rc.settings, "upload_retention_days", 30)
    assert rc.get("upload_retention_days") == 30


def test_nao_deixa_lixo_tmp_ao_gravar():
    rc.aplicar({"editor": "vim"})
    tmps = list(rc._caminho().parent.glob("*.tmp"))
    assert tmps == []


def test_editor_nao_aceita_caminho():
    """O editor vira argv[0] de subprocess. Nome nu (code, nvim) sim; caminho solto, não."""
    rc.aplicar({"editor": "nvim"})
    assert rc.get("editor") == "nvim"
    for ruim in ["/tmp/evil.sh", "../../bin/sh", "-flag", "sub/dir/bin"]:
        with pytest.raises(ValueError):
            rc.aplicar({"editor": ruim})
    assert rc.get("editor") == "nvim"       # nenhuma tentativa ruim passou


def test_llm_base_url_recusa_esquema_nao_http():
    """Mesmo argumento do editor: antes só o dono da máquina escolhia o endpoint (env), agora o
    celular escreve — só vazio (volta ao padrão) ou http(s):// de verdade."""
    rc.aplicar({"llm_base_url": "https://x/v1"})
    with pytest.raises(ValueError):
        rc.aplicar({"llm_base_url": "ftp://x"})
    assert rc.get("llm_base_url") == "https://x/v1"     # a tentativa ruim não passou


def test_llm_base_url_aceita_http_e_vazio():
    rc.aplicar({"llm_base_url": "https://x/v1"})
    assert rc.get("llm_base_url") == "https://x/v1"
    rc.aplicar({"llm_base_url": ""})
    assert rc.get("llm_base_url") == ""


def test_endpoint_de_transcricao_aceita_http_e_recusa_outro_esquema():
    rc.aplicar({"transcription_base_url": "https://fala.exemplo/v1"})
    assert rc.get("transcription_base_url") == "https://fala.exemplo/v1"
    with pytest.raises(ValueError):
        rc.aplicar({"transcription_base_url": "ftp://fala.exemplo"})
    assert rc.get("transcription_base_url") == "https://fala.exemplo/v1"


def test_vocabulario_grande_demais_e_RECUSADO_na_gravacao():
    """O teto tem que doer na hora de salvar, nao na hora de transcrever. Sem isto a tela diz
    "salvo", o corte acontece calado depois, e os nomes que a pessoa cadastrou pra parar de sair
    errado continuam saindo errado sem nenhuma explicacao em lugar nenhum."""
    from app.transcribe import VOCAB_USUARIO_MAX
    with pytest.raises(ValueError) as ei:
        rc._coagir("ditado_vocabulario", "x" * (VOCAB_USUARIO_MAX + 1))
    assert str(VOCAB_USUARIO_MAX) in str(ei.value)


def test_vocabulario_no_limite_passa():
    from app.transcribe import VOCAB_USUARIO_MAX
    texto = "x" * VOCAB_USUARIO_MAX
    assert rc._coagir("ditado_vocabulario", texto) == texto


def test_scan_roots_recusa_entrada_que_nao_e_diretorio(tmp_path):
    """Vindo da tela, o descarte calado do resolve_scan_roots viraria "salvei e o chip nunca
    apareceu" — a gravacao recusa nomeando a entrada ruim."""
    ok = tmp_path / "projetos"
    ok.mkdir()
    with pytest.raises(ValueError) as ei:
        rc.aplicar({"scan_roots": f"{ok},{tmp_path / 'nao-existe'}"})
    assert "nao-existe" in str(ei.value)
    assert rc.get("scan_roots") is None or "nao-existe" not in str(rc.get("scan_roots"))


def test_scan_roots_aceita_diretorios_e_vazio(tmp_path):
    a = tmp_path / "a"
    a.mkdir()
    rc.aplicar({"scan_roots": str(a)})
    assert rc.get("scan_roots") == str(a)
    rc.aplicar({"scan_roots": ""})       # vazio = volta ao env (resolve_scan_roots cai no CP_SCAN_ROOTS)
    assert rc.get("scan_roots") == ""


def test_env_jev_desligado_leva_so_o_marcador():
    """A chave NÃO entra numa sessão aberta com o recurso desligado — e o marcador vai mesmo assim,
    pra o `hangar-preview objetivo` separar "desligado aqui" de "nunca configurado"."""
    rc.aplicar({"jev_api_key": "ts-secreta", "jev_texto_cmd": "meu-llm"})
    assert rc.env_jev(False) == {"HANGAR_JEV": "off"}


def test_env_jev_ligado_leva_chave_e_o_texto_configurado():
    rc.aplicar({"jev_api_key": "ts-secreta", "jev_texto_base_url": "https://llm.exemplo",
                "jev_texto_api_key": "k", "jev_texto_modelo": "mini"})
    env = rc.env_jev(True)
    assert env["HANGAR_JEV"] == "on"
    assert env["TYPESAFE_API_KEY"] == "ts-secreta"
    assert env["JEV_TEXTO_BASE_URL"] == "https://llm.exemplo"
    assert env["JEV_TEXTO_MODELO"] == "mini"
    # Campo vazio não vira variável vazia: o CLI decide pela AUSÊNCIA, e "" ligaria o ramo errado.
    assert "JEV_TEXTO_CMD" not in env


def test_env_jev_ligado_leva_endpoint_e_modelo_do_jev():
    rc.aplicar({"jev_api_key": "sk-or-x", "jev_endpoint": "https://openrouter.ai/api/alpha/decisions",
                "jev_model": "typesafe/jev-1.13-20260917"})
    env = rc.env_jev(True)
    assert env["JEV_ENDPOINT"] == "https://openrouter.ai/api/alpha/decisions"
    assert env["JEV_MODEL"] == "typesafe/jev-1.13-20260917"


def test_env_jev_desligado_nao_leva_endpoint_nem_modelo():
    rc.aplicar({"jev_endpoint": "https://openrouter.ai/api/alpha/decisions", "jev_model": "m"})
    assert rc.env_jev(False) == {"HANGAR_JEV": "off"}


def test_endpoint_do_jev_exige_url_http():
    with pytest.raises(ValueError, match="jev_endpoint"):
        rc.aplicar({"jev_endpoint": "openrouter.ai/api/alpha/decisions"})
    rc.aplicar({"jev_endpoint": ""})


def test_env_jev_ligado_sem_chave_cadastrada_nao_inventa_variavel():
    assert rc.env_jev(True) == {"HANGAR_JEV": "on"}


def test_function_hooks_desligado_nao_poe_variavel_nenhuma():
    """Sem marcador `off`, ao contrário do Jev: a AUSÊNCIA da variável é o desligado, e é ela que o
    Claude Code lê pra decidir se carrega plugin de function hook."""
    assert rc.env_function_hooks() == {}


def test_function_hooks_ligado_abre_o_portao():
    rc.aplicar({"claude_function_hooks": True})
    assert rc.env_function_hooks() == {"CLAUDE_CODE_ENABLE_FUNCTION_HOOKS": "1"}


def test_function_hooks_le_a_configuracao_atual_e_nao_a_do_nascimento():
    """Diferente do `jev`, que preserva a escolha da sessão: aqui não houve escolha por sessão, há
    configuração de servidor — desligar e relançar tem que devolver a sessão sem o portão."""
    rc.aplicar({"claude_function_hooks": True})
    assert rc.env_function_hooks()
    rc.aplicar({"claude_function_hooks": False})
    assert rc.env_function_hooks() == {}


def test_chaves_do_jev_nunca_voltam_inteiras():
    """Cadastradas em SEGREDOS de propósito, e não pelo acaso de o nome ter `_key`."""
    assert {"jev_api_key", "jev_texto_api_key"} <= rc.SEGREDOS
    rc.aplicar({"jev_api_key": "ts-1234567890"})
    assert rc.estado()["jev_api_key"]["valor"] != "ts-1234567890"


