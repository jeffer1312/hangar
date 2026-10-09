from types import SimpleNamespace
import app.doctor as doctor


def _settings(**kw):
    base = dict(auth_token="segredo-forte", port=8765, public_url="", lan_bind_ip="0.0.0.0")
    base.update(kw)
    return SimpleNamespace(**base)


def _tudo_ok(monkeypatch):
    monkeypatch.setattr(doctor, "_porta_responde", lambda porta: True)
    monkeypatch.setattr(doctor, "_tempo_resposta", lambda porta: 0.01)
    monkeypatch.setattr(doctor, "_reinicios_automaticos", lambda dias=7: [])
    monkeypatch.setattr(doctor, "_prioridade_backend", lambda porta: "normal")
    monkeypatch.setattr(doctor, "_binario", lambda nome: "/usr/bin/x")
    monkeypatch.setattr(doctor, "_claude_logado", lambda: True)
    monkeypatch.setattr(doctor, "_tailscale", lambda: ("instalado", "logado"))
    monkeypatch.setattr(doctor, "_lan_responde", lambda s: True)


def test_token_de_fabrica_e_erro(monkeypatch):
    _tudo_ok(monkeypatch)
    linhas = doctor.diagnosticar(_settings(auth_token="change-me"))
    erro = [l for l in linhas if l.nivel == "erro"]
    assert erro and "token" in erro[0].titulo.lower()
    assert "backend/.env" in erro[0].conserto


def test_tudo_ok_sai_zero(monkeypatch, capsys):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "settings", _settings(public_url="https://pc.tail.ts.net"))
    assert doctor.main([]) == 0
    saida = capsys.readouterr().out
    assert "X   " not in saida and "ok  " in saida


def test_backend_fora_do_ar_e_erro_com_conserto_do_so(monkeypatch):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_porta_responde", lambda porta: False)
    linhas = doctor.diagnosticar(_settings())
    porta = next(l for l in linhas if "8765" in l.titulo)
    assert porta.nivel == "erro"
    assert ("systemctl --user restart hangar-backend" in porta.conserto
            or "Start-ScheduledTask" in porta.conserto)


def test_sem_tailscale_e_aviso_nao_erro(monkeypatch):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_tailscale", lambda: ("ausente", ""))
    linhas = doctor.diagnosticar(_settings())
    ts = next(l for l in linhas if "Tailscale" in l.titulo)
    assert ts.nivel == "aviso"


def test_lan_responde_le_o_shape_real_do_alcance(monkeypatch):
    # shape real de levantar_estados: `tipo` + `estado` ("ok"/"falhou"); NÃO existe chave `ok`.
    import app.alcance as alcance
    monkeypatch.setattr(alcance, "levantar_estados", lambda s: {"enderecos": [
        {"tipo": "nesta_maquina", "url": "http://127.0.0.1:8765", "estado": "ok"},
        {"tipo": "rede_local", "url": "http://10.0.0.5:8765", "estado": "ok"},
    ]})
    assert doctor._lan_responde(_settings()) is True
    monkeypatch.setattr(alcance, "levantar_estados", lambda s: {"enderecos": [
        {"tipo": "rede_local", "url": "http://10.0.0.5:8765", "estado": "falhou", "motivo": "timeout"},
    ]})
    assert doctor._lan_responde(_settings()) is False


def test_claude_logado_pergunta_a_cli_da_ativa_e_das_contas_com_credencial(monkeypatch, tmp_path):
    """O doctor roda sem o backend de pé: o login vem da CLI. A conta ativa vem primeiro, conta
    sem `.credentials.json` não é perguntada, e a primeira logada encerra a busca."""
    import json
    import subprocess
    from app import contas
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    ativa = (tmp_path / ".claude")
    ativa.mkdir()
    contas_criadas = {}
    for nome, credencial in (("trabalho", True), ("vazia", False)):
        conta = tmp_path / f".claude-{nome}"
        conta.mkdir()
        (conta / contas.MARCADOR).write_text("", encoding="utf-8")
        if credencial:
            (conta / ".credentials.json").write_text("{}", encoding="utf-8")
        contas_criadas[nome] = str(conta.resolve())
    monkeypatch.setattr(doctor, "_binario", lambda nome: "/fake/" + nome)
    logadas, perguntas = set(), []

    def cli(argv, **kwargs):
        assert argv == ["/fake/claude", "auth", "status", "--json"]
        perguntas.append(kwargs["env"]["CLAUDE_CONFIG_DIR"])
        saida = json.dumps({"loggedIn": kwargs["env"]["CLAUDE_CONFIG_DIR"] in logadas})
        return subprocess.CompletedProcess(argv, 1, saida, "")

    monkeypatch.setattr(subprocess, "run", cli)
    assert doctor._claude_logado() is False
    assert perguntas == [str(ativa.resolve()), contas_criadas["trabalho"]]
    perguntas.clear()
    logadas.add(str(ativa.resolve()))
    assert doctor._claude_logado() is True
    assert perguntas == [str(ativa.resolve())]


def test_erro_sai_um_e_aviso_nao(monkeypatch, capsys):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_porta_responde", lambda porta: False)
    monkeypatch.setattr(doctor, "settings", _settings(public_url="https://pc.tail.ts.net"))
    assert doctor.main([]) == 1
    assert "para consertar" in capsys.readouterr().out

    # Só avisos: rc 0, mas a última linha não pode dizer "tudo certo".
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_tailscale", lambda: ("ausente", ""))
    assert doctor.main([]) == 0
    saida = capsys.readouterr().out
    assert "tudo certo" not in saida
    assert "aviso(s)" in saida and "nada quebrado" in saida


def test_so_codex_sem_claude_nao_e_erro(monkeypatch):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_binario", lambda nome: None if nome == "claude" else "/usr/bin/x")
    linhas = doctor.diagnosticar(_settings())
    assert not [l for l in linhas if l.nivel == "erro"]
    agentes = next(l for l in linhas if "agentes de código" in l.titulo)
    assert agentes.nivel == "ok" and "Codex" in agentes.titulo


def test_nenhum_agente_e_erro(monkeypatch):
    _tudo_ok(monkeypatch)
    sem = {"claude", "codex", "pi", "omp", "kimi"}
    monkeypatch.setattr(doctor, "_binario", lambda nome: None if nome in sem else "/usr/bin/x")
    linha = next(l for l in doctor.diagnosticar(_settings()) if "nenhum agente" in l.titulo)
    assert linha.nivel == "erro" and "claude.ai/install" in linha.conserto


def test_ascii_para_o_console_do_windows():
    assert doctor._ascii("endereço — não") == "endereco - nao"


def test_qr_sem_terminal_devolve_2_e_so_a_url(monkeypatch, capsys):
    # `--qr` não diagnostica nada: é só o QR/URL de pareamento.
    monkeypatch.setattr(doctor, "diagnosticar", lambda s: 1 / 0)
    monkeypatch.setattr(doctor, "settings", _settings(public_url="https://pc.tail.ts.net"))
    assert doctor.main(["--qr"]) == 2   # pytest captura o stdout -> não é tty
    saida = capsys.readouterr().out
    assert "https://pc.tail.ts.net/?token=segredo-forte" in saida
    assert "█" not in saida


def test_porta_aberta_sem_resposta_e_travado(monkeypatch):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_tempo_resposta", lambda porta: None)
    linha = next(l for l in doctor.diagnosticar(_settings()) if "travado" in l.titulo)
    assert linha.nivel == "erro"


def test_resposta_acima_de_3s_e_aviso_de_lento(monkeypatch):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_tempo_resposta", lambda porta: 4.2)
    linha = next(l for l in doctor.diagnosticar(_settings()) if "lento" in l.titulo)
    assert linha.nivel == "aviso" and "4.2 s" in linha.titulo


def test_reinicios_da_vigia_do_windows_contam_so_o_periodo(monkeypatch, tmp_path):
    import datetime as dt
    import app.log_paths as log_paths
    (tmp_path / "privado").mkdir()
    agora = dt.datetime.now()
    velho = (agora - dt.timedelta(days=30)).isoformat(timespec="seconds")
    novo = (agora - dt.timedelta(hours=1)).isoformat(timespec="seconds")
    (tmp_path / "privado" / "hangar-vigia.log").write_text(
        f"\ufeff{velho} vigia: hangar-backend sem resposta HTTP; recuperando a instancia identificada\n"
        f"{novo} vigia: hangar-frontend sem resposta HTTP; recuperando a instancia identificada\n"
        f"{novo} vigia: hangar-backend sem resposta HTTP; recuperando a instancia identificada\n",
        encoding="utf-8")
    monkeypatch.setattr(doctor, "_WIN", True)
    monkeypatch.setattr(log_paths, "base", lambda: tmp_path)
    assert doctor._reinicios_automaticos() == [novo]


def test_windows_avisa_backend_abaixo_do_normal(monkeypatch):
    _tudo_ok(monkeypatch)
    monkeypatch.setattr(doctor, "_WIN", True)
    monkeypatch.setattr(doctor, "_prioridade_backend", lambda porta: "abaixo do normal")
    linha = next(l for l in doctor.diagnosticar(_settings()) if "prioridade" in l.titulo)
    assert linha.nivel == "aviso" and "install.ps1 -Update" in linha.conserto
