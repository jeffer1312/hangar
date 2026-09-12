from pathlib import Path
from app.config import _default_projects_dir, detect_lan_ip, pairing_url_api, resolve_bind_ip, pairing_url, Settings
from app.config import SEGREDOS_DO_ENV, VARIAVEIS_DO_AMBIENTE, variaveis_env
from app.config import (
    _default_projects_dir,
    detect_lan_ip,
    pairing_url,
    porta_do_front,
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


def test_pairing_url_uses_public_url_when_set():
    s = Settings(public_url="https://pocket.local/", auth_token="tok")
    assert pairing_url(s) == "https://pocket.local/?token=tok"


def test_pairing_url_builds_from_bind_ip_and_front_port():
    # The QR points at the PWA front (front_port), not the API port.
    # public_url="" explicito: hermetico contra um backend/.env local que defina CP_PUBLIC_URL
    # (senao o fallback por bind-ip nao seria exercitado).
    s = Settings(lan_bind_ip="192.168.1.50", front_port=5173, auth_token="tok", public_url="")
    assert pairing_url(s) == "http://192.168.1.50:5173/?token=tok"


def test_pairing_url_api_usa_porta_do_backend():
    # public_url="" de propósito: backend/.env desta máquina tem CP_PUBLIC_URL e o Settings herda
    s = Settings(lan_bind_ip="10.0.0.5", port=8765, auth_token="t", public_url="")
    assert pairing_url_api(s) == "http://10.0.0.5:8765/?token=t"


def test_pairing_url_api_com_public_url_leva_api_na_query():
    s = Settings(lan_bind_ip="10.0.0.5", port=8765, auth_token="t", public_url="https://casa.ts.net/")
    assert pairing_url_api(s) == "https://casa.ts.net/?token=t&api=http://10.0.0.5:8765"
def test_porta_do_front_cai_no_backend_quando_nao_ha_servico_de_front():
    # Sem serviço de front instalado (o padrão desde que o backend passou a servir o dist), o QR
    # e o painel de alcance têm de apontar pra porta do BACKEND. Com 5173 cravado como default,
    # os dois mandavam a pessoa pra uma porta onde ninguém escuta.
    # front_port=0 explícito: hermético contra um backend/.env local com CP_FRONT_PORT=5173
    # (quem mantém o preview tem isso gravado, e o teste passava só no CI, que não tem .env).
    assert porta_do_front(Settings(port=8765, front_port=0)) == 8765
    assert porta_do_front(Settings(port=9000, front_port=0)) == 9000
    assert porta_do_front(Settings(port=8765, front_port=5173)) == 5173


def test_front_port_vazio_nao_derruba_o_backend():
    # `CP_FRONT_PORT=` no .env levantava ValidationError, e como `settings = Settings()` roda no
    # import do módulo, o backend inteiro não subia — sem tela e sem mensagem que explicasse.
    assert porta_do_front(Settings(front_port="", port=8765)) == 8765  # type: ignore[arg-type]
    assert porta_do_front(Settings(front_port="  ", port=8765)) == 8765  # type: ignore[arg-type]


def test_pairing_url_usa_a_porta_do_backend_sem_front_port():
    s = Settings(lan_bind_ip="192.168.1.50", auth_token="tok", public_url="", port=8765, front_port=0)
    assert pairing_url(s) == "http://192.168.1.50:8765/?token=tok"


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
_TOKEN_HTTP = "tok-auth-que-nao-pode-vazar"
_AUTH_HTTP = {"Authorization": f"Bearer {_TOKEN_HTTP}"}


def _cliente_http(monkeypatch):
    """Mesmo cliente e mesma auth do `test_api_atualizacao.py` — o prior art que a spec cita.

    A única diferença é o `monkeypatch` no lugar da atribuição direta ao singleton: este arquivo
    roda junto dos testes de `Settings`, e um token deixado gravado ali vazaria para eles."""
    from fastapi.testclient import TestClient
    from app.config import settings as singleton
    monkeypatch.setattr(singleton, "auth_token", _TOKEN_HTTP)
    from app.api import app
    return TestClient(app)


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


def test_campos_que_mudam_comportamento_visivel_tem_codigo_de_descricao():
    # As 8 com descrição são as 5 lidas do ambiente (testadas acima) mais estas três. As demais
    # (porta, IP, VAPID, sync, proxy) ficam sem: o nome se explica, e a decisão do usuário foi não
    # criar dezenas de entradas de i18n numa seção de leitura.
    por_nome = _por_nome(variaveis_env(_settings()))
    assert por_nome["CP_AUTO_RESUME"]["descricao"] == "auto_resume"
    assert por_nome["CP_OMP_PLUGIN_SYNC_ENABLED"]["descricao"] == "omp_plugin_sync"
    assert por_nome["CP_OMP_CLAUDE_CONTEXT_ENABLED"]["descricao"] == "omp_claude_context"
    assert por_nome["CP_PORT"]["descricao"] is None
    assert por_nome["CP_VAPID_SUBJECT"]["descricao"] is None
    # São OITO, nem mais nem menos: a lista é decisão do usuário, não algo que cresce sozinho.
    com_descricao = [v["nome"] for v in variaveis_env(_settings()) if v["descricao"]]
    assert len(com_descricao) == 8, com_descricao


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


def test_rota_de_config_entrega_as_variaveis_do_env_sem_vazar_segredo(monkeypatch):
    """Costura da ROTA, não da função: o que este teste segura é a linha de `get_config` que liga a
    lista à resposta, e a regra de a chave ser IRMÃ de `somente_leitura`.

    Sem ele, apagar aquela linha — ou mover a chave para dentro do bloco só-leitura — deixa pytest,
    vitest e `check` os três verdes enquanto a seção some da tela: o front lê `variaveis_env ?? []`,
    a lista vazia esconde a seção, o campo é opcional no tipo, e todo teste de componente injeta a
    lista já pronta pelo mock do store. Medido: com a linha removida, a bateria inteira passava."""
    from app import runtime_config
    from app.config import settings as singleton

    segredos = {
        "auth_token": _TOKEN_HTTP,
        "vapid_private": "vp-nao-pode-vazar",
        "sync_bootstrap": "sb-nao-pode-vazar",
        "sync_session_secret": "sss-nao-pode-vazar",
        "deploy_secret": "ds-nao-pode-vazar",
    }
    for campo, valor in segredos.items():
        monkeypatch.setattr(singleton, campo, valor)
    monkeypatch.setenv("CP_CODEX_SYNC_ENABLED", "0")

    r = _cliente_http(monkeypatch).get("/api/config", headers=_AUTH_HTTP)
    assert r.status_code == 200
    corpo = r.json()

    # (a) a lista chega PELA ROTA, não só pela função.
    assert isinstance(corpo.get("variaveis_env"), list), "a rota não devolveu a lista"
    assert corpo["variaveis_env"], "a rota devolveu a lista vazia"

    # (b) chave irmã de `somente_leitura`, nunca dentro dele: aquele bloco é um mapa simples,
    # desenhado linha a linha, e uma lista ali dentro vira "[object Object]" na tela.
    assert "variaveis_env" not in corpo["somente_leitura"]

    nomes = {v["nome"] for v in corpo["variaveis_env"]}

    # (c) lista completa: todo campo do Settings que a tela não edita, mais as lidas do ambiente.
    faltando = {f"CP_{campo.upper()}" for campo in Settings.model_fields
                if campo not in runtime_config.EDITAVEIS} - nomes
    assert not faltando, f"campos do Settings fora da resposta: {sorted(faltando)}"
    assert set(VARIAVEIS_DO_AMBIENTE) <= nomes

    # (d) segredo só como definida/não definida — e o valor em lugar NENHUM do corpo serializado,
    # não só dentro do item dele.
    por_nome = _por_nome(corpo["variaveis_env"])
    for campo in SEGREDOS_DO_ENV:
        v = por_nome[f"CP_{campo.upper()}"]
        assert v["segredo"] is True
        assert v["valor"] is None, f"{campo} devolveu valor pela rota"
        assert v["definida"] is True
    for valor in segredos.values():
        assert valor not in r.text, "valor de segredo no corpo da resposta"

    # (e) chave editável pela tela não entra: estaria em duplicidade e com a etiqueta errada.
    for campo in ("groq_api_key", "automations", "codex_sync"):
        assert f"CP_{campo.upper()}" not in nomes

    # o aviso do kill-switch também atravessa a rota, não só a função.
    assert por_nome["CP_CODEX_SYNC_ENABLED"]["alerta"] == "codex_sync_desligado"


def test_nome_da_variavel_respeita_o_alias_declarado_no_campo():
    # Remontar "CP_<CAMPO>" na mão mostraria um nome que, escrito no .env, não faz efeito nenhum.
    from pydantic import AliasChoices, Field

    class SettingsComAlias(Settings):
        atalho: str = Field("", validation_alias=AliasChoices("CP_NOME_DE_VERDADE", "NOME_CURTO"))

    nomes = {v["nome"] for v in variaveis_env(SettingsComAlias(_env_file=None))}
    assert "CP_NOME_DE_VERDADE" in nomes
    assert "CP_ATALHO" not in nomes
