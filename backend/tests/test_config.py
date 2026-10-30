from pathlib import Path

import pytest
from app.config import _default_projects_dir, detect_lan_ip, resolve_bind_ip, Settings
from app.config import SEGREDOS_DO_ENV, VARIAVEIS_DO_AMBIENTE, variaveis_env
from app.config import (
    _default_projects_dir,
    detect_lan_ip,
    resolve_bind_ip,
    Settings,
)


def test_default_projects_dir_honors_claude_config_dir(monkeypatch):
    """The transcript dir must follow $CLAUDE_CONFIG_DIR, not a hardcoded ~/.claude —
    machines/users set CLAUDE_CONFIG_DIR to different locations."""
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/tmp/some-custom-config")
    assert _default_projects_dir() == Path("/tmp/some-custom-config/projects")


def test_default_projects_dir_falls_back_to_home(monkeypatch):
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    assert _default_projects_dir() == Path.home() / ".claude" / "projects"


def test_detect_lan_ip_returns_ipv4():
    ip = detect_lan_ip()
    assert isinstance(ip, str) and ip.count(".") == 3


def test_resolve_bind_ip_passthrough_when_not_auto():
    assert resolve_bind_ip(Settings(lan_bind_ip="192.168.1.50")) == "192.168.1.50"














# ── Variáveis do .env em Avançado ────────────────────────────────────────────────────────────
# A tela só mostra; a costura que ela não alcança (a lista está completa, segredo não sai, chave
# editável não entra) fica aqui.

def _por_nome(lista):
    return {v["nome"]: v for v in lista}


def _settings(**kw):
    """Settings HERMÉTICO: sem ler o `backend/.env` da máquina. Sem isso o teste passa ou falha
    conforme o arquivo local — o mesmo cuidado que os testes de `pairing_url` já tomam à mão."""
    return Settings(_env_file=None, **kw)


# Token distinto de qualquer palavra que apareça num NOME de variável: a asserção de vazamento
# procura o valor no texto cru da resposta, e um "secret" genérico casaria com "CP_DEPLOY_SECRET".




def test_variaveis_env_nao_devolve_valor_de_segredo(monkeypatch):
    # Nem mascarado: a tela não edita nenhuma destas cinco, então mostrar pedaço delas seria
    # vazamento sem uso em troca. Um segredo real no Settings tem de sair da API só como `definida`.
    monkeypatch.delenv("CP_CODEX_SYNC_ENABLED", raising=False)
    s = _settings(auth_token="tok-secreto-123", vapid_private="vp-secreto",
                  sync_bootstrap="sb-secreto", sync_session_secret="sss-secreto",
                  deploy_secret="ds-secreto")
    por_nome = _por_nome(variaveis_env(s))
    for campo in SEGREDOS_DO_ENV:
        v = por_nome[f"CP_{campo.upper()}"]
        assert v["segredo"] is True
        assert v["valor"] is None, f"{campo} devolveu valor"
        assert v["definida"] is True
    # E nenhum dos segredos aparece em lugar nenhum da estrutura serializada.
    bruto = repr(variaveis_env(s))
    for pedaco in ("tok-secreto-123", "vp-secreto", "sb-secreto", "sss-secreto", "ds-secreto"):
        assert pedaco not in bruto


def test_variaveis_env_marca_segredo_ausente_como_nao_definida():
    # "não definida" tem de ser distinguível de "definida" — é a única informação que a tela tem
    # sobre um segredo, e sem isso a linha diria o mesmo nos dois casos.
    por_nome = _por_nome(variaveis_env(_settings(deploy_secret="")))
    assert por_nome["CP_DEPLOY_SECRET"]["definida"] is False
    assert por_nome["CP_DEPLOY_SECRET"]["valor"] is None


def test_variaveis_env_nao_inclui_chave_editavel_pela_tela():
    # Com uma chave editável aqui, a mesma configuração apareceria duas vezes na tela, e a segunda
    # diria que exige reiniciar — texto falso que nenhuma outra verificação pegaria.
    from app import runtime_config
    nomes = {v["nome"] for v in variaveis_env(_settings())}
    for campo in runtime_config.EDITAVEIS:
        assert f"CP_{campo.upper()}" not in nomes, f"{campo} é editável pela tela e vazou pra lista"


def test_variaveis_env_traz_os_campos_do_settings_que_a_tela_nao_edita():
    from app import runtime_config
    nomes = {v["nome"] for v in variaveis_env(_settings())}
    esperados = {f"CP_{c.upper()}" for c in Settings.model_fields if c not in runtime_config.EDITAVEIS}
    assert esperados <= nomes


def test_variaveis_env_traz_as_lidas_direto_do_ambiente_com_codigo_de_descricao(monkeypatch):
    monkeypatch.setenv("CP_TERMINAL", "kitty")
    por_nome = _por_nome(variaveis_env(_settings()))
    for nome, codigo in VARIAVEIS_DO_AMBIENTE.items():
        assert nome in por_nome, f"{nome} ficou fora da lista"
        assert por_nome[nome]["descricao"] == codigo
    assert por_nome["CP_TERMINAL"]["valor"] == "kitty"
    assert por_nome["CP_TERMINAL"]["definida"] is True


def test_toda_variavel_listada_tem_codigo_de_descricao():
    # Uma variável sem frase é o `.env` copiado pra tela: campo novo no Settings que não for
    # editável pela tela entra na lista sozinho, e este teste é o que obriga a frase a nascer junto.
    por_nome = _por_nome(variaveis_env(_settings()))
    assert por_nome["CP_AUTO_RESUME"]["descricao"] == "auto_resume"
    assert por_nome["CP_OMP_PLUGIN_SYNC_ENABLED"]["descricao"] == "omp_plugin_sync"
    assert por_nome["CP_STALL_POLL_SECONDS"]["descricao"] == "stall_poll_seconds"
    sem_descricao = [v["nome"] for v in variaveis_env(_settings()) if not v["descricao"]]
    assert sem_descricao == []


def test_variaveis_env_le_o_ambiente_e_nao_o_settings(monkeypatch):
    # Estas cinco não moram no Settings: quem as lê (api.py, codex_integracao, engines, pricing)
    # olha `os.environ`. Mostrar outra fonte afirmaria um valor que o código não usa.
    monkeypatch.delenv("CP_PRICING_OFFLINE", raising=False)
    assert _por_nome(variaveis_env(_settings()))["CP_PRICING_OFFLINE"]["definida"] is False
    monkeypatch.setenv("CP_PRICING_OFFLINE", "1")
    assert _por_nome(variaveis_env(_settings()))["CP_PRICING_OFFLINE"]["definida"] is True


def test_kill_switch_do_codex_desligado_avisa(monkeypatch):
    # Desligado, ele ANULA o toggle "Sincronização automática" de Harnesses — sem o aviso o toggle
    # parece quebrado, que é a queixa que originou a linha.
    for valor in ("0", "false", "NO"):
        monkeypatch.setenv("CP_CODEX_SYNC_ENABLED", valor)
        v = _por_nome(variaveis_env(_settings()))["CP_CODEX_SYNC_ENABLED"]
        assert v["alerta"] == "codex_sync_desligado", f"{valor} não gerou aviso"


def test_kill_switch_do_codex_ligado_ou_ausente_nao_avisa(monkeypatch):
    monkeypatch.setenv("CP_CODEX_SYNC_ENABLED", "1")
    assert _por_nome(variaveis_env(_settings()))["CP_CODEX_SYNC_ENABLED"]["alerta"] is None
    monkeypatch.delenv("CP_CODEX_SYNC_ENABLED", raising=False)
    assert _por_nome(variaveis_env(_settings()))["CP_CODEX_SYNC_ENABLED"]["alerta"] is None
    # E o aviso é só dele: nenhuma outra variável carrega alerta.
    outras = [v for v in variaveis_env(_settings()) if v["nome"] != "CP_CODEX_SYNC_ENABLED"]
    assert all(v["alerta"] is None for v in outras)


def test_variaveis_env_serializa_caminho_como_texto():
    # `projects_dir` e `sync_data` são Path. Sem virar texto, o JSON da resposta quebra (e, se
    # passasse, a tela desenharia um objeto em vez do caminho).
    por_nome = _por_nome(variaveis_env(_settings(projects_dir=Path("/tmp/p"))))
    assert por_nome["CP_PROJECTS_DIR"]["valor"] == "/tmp/p"
    assert isinstance(por_nome["CP_SYNC_DATA"]["valor"], str)


def test_variaveis_env_trata_booleano_falso_e_zero_como_definidos():
    # `False` e `0` são valores, não ausências: a linha tem de dizer "não" e "0", não "—".
    por_nome = _por_nome(variaveis_env(_settings(auto_resume=False, stall_poll_seconds=0)))
    assert por_nome["CP_AUTO_RESUME"]["valor"] is False
    assert por_nome["CP_AUTO_RESUME"]["definida"] is True
    assert por_nome["CP_STALL_POLL_SECONDS"]["valor"] == 0
    assert por_nome["CP_STALL_POLL_SECONDS"]["definida"] is True


def test_campo_novo_com_cara_de_segredo_ja_nasce_escondido():
    # A lista explícita depende de alguém lembrar de cadastrar. Um campo novo chamado `*_secret`,
    # `*_token` e afins tem de sair sem valor mesmo esquecido nela — senão o vazamento entra por
    # um commit que ninguém liga a esta função.
    class SettingsComSegredoNovo(Settings):
        webhook_secret: str = ""
        parceiro_token: str = ""
        cofre_password: str = ""
        porta_alternativa: int = 9999   # controle: não tem cara de segredo, o valor aparece

    s = SettingsComSegredoNovo(_env_file=None, webhook_secret="wh-secreto",
                               parceiro_token="pt-secreto", cofre_password="cp-secreto")
    por_nome = _por_nome(variaveis_env(s))
    for nome in ("CP_WEBHOOK_SECRET", "CP_PARCEIRO_TOKEN", "CP_COFRE_PASSWORD"):
        assert por_nome[nome]["segredo"] is True, f"{nome} não foi reconhecido como segredo"
        assert por_nome[nome]["valor"] is None
        assert por_nome[nome]["definida"] is True
    assert por_nome["CP_PORTA_ALTERNATIVA"]["segredo"] is False
    assert por_nome["CP_PORTA_ALTERNATIVA"]["valor"] == 9999
    for pedaco in ("wh-secreto", "pt-secreto", "cp-secreto"):
        assert pedaco not in repr(variaveis_env(s))






def test_nome_da_variavel_respeita_o_alias_declarado_no_campo():
    # Remontar "CP_<CAMPO>" na mão mostraria um nome que, escrito no .env, não faz efeito nenhum.
    from pydantic import AliasChoices, Field

    class SettingsComAlias(Settings):
        atalho: str = Field("", validation_alias=AliasChoices("CP_NOME_DE_VERDADE", "NOME_CURTO"))

    nomes = {v["nome"] for v in variaveis_env(SettingsComAlias(_env_file=None))}
    assert "CP_NOME_DE_VERDADE" in nomes
    assert "CP_ATALHO" not in nomes


@pytest.mark.parametrize("valor", ["-x", "a..b", "com espaco", "x;rm", "a" * 101])
def test_update_branch_invalida_vira_main_e_vai_pro_diario(monkeypatch, valor):
    from app import diag
    eventos = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **c: eventos.append((evento, nivel)))
    assert Settings(update_branch=valor).update_branch == ""
    assert eventos == [("atualizacao.branch_invalida", "aviso")]


def test_update_branch_valida_vem_do_ambiente(monkeypatch):
    monkeypatch.setenv("CP_UPDATE_BRANCH", " feat/hangar-server_1.2 ")
    assert Settings().update_branch == "feat/hangar-server_1.2"
    monkeypatch.setenv("CP_UPDATE_BRANCH", "")
    assert Settings().update_branch == ""


def test_invalid_update_last_branch_reports_correct_variable(monkeypatch, caplog):
    from app import diag
    events = []
    monkeypatch.setattr(diag, "registrar", lambda event, level="ok", **fields: events.append(fields))
    result = Settings(update_branch="test/channel", update_last_branch="bad..branch", _env_file=None)
    assert result.update_branch == "test/channel"
    assert result.update_last_branch == ""
    assert "CP_UPDATE_LAST_BRANCH" in caplog.text
    assert events[-1]["detalhe"] == "CP_UPDATE_LAST_BRANCH ignorado"


def test_settings_read_the_env_from_the_user_data_folder(monkeypatch, tmp_path):
    """Máquina migrada: o `.env` está só em `~/.hangar/config/` e o token sai de lá."""
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CP_AUTH_TOKEN", raising=False)
    config = tmp_path / ".hangar" / "config"
    config.mkdir(parents=True)
    (config / ".env").write_text("CP_AUTH_TOKEN=do-lugar-novo\n", encoding="utf-8")
    # Comparado fora do assert: na falha o pytest imprimiria o token lido, que pode ser o real.
    veio_do_lugar_novo = Settings().auth_token == "do-lugar-novo"
    assert veio_do_lugar_novo, "o token não veio de ~/.hangar/config/.env"


def test_settings_keep_reading_backend_env_on_an_unmigrated_machine(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    from app import user_data
    assert user_data.path(".env") == Path(__file__).resolve().parent.parent / ".env"
    assert not (tmp_path / ".hangar").exists()
