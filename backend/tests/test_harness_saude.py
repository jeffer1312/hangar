"""Painel de saúde dos harnesses (app/harness_saude.py): checagem lê, conserto reusa o instalador."""
import json
import pytest
from pathlib import Path

from app import harness_saude as h


def test_extensoes_faltando_e_o_conserto_liga(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    (repo / "scripts" / "pi" / "lib").mkdir(parents=True)
    for nome in h._EXTENSOES_PI:
        (repo / "scripts" / "pi" / f"{nome}.ts").write_text("")
    monkeypatch.setattr(h, "_REPO", repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    raiz = tmp_path / ".pi" / "agent"
    (raiz / "extensions").mkdir(parents=True)
    # Arquivo real do usuário com o mesmo nome: é dele, não é falta nem vira symlink.
    (raiz / "extensions" / "claude-todo.ts").write_text("meu")
    item = h._extensoes("pi")
    assert item["ok"] is False and item["codigo"] == "faltam" and item["conserto"] == "extensoes:pi"
    assert "claude-todo" not in item["params"]["lista"] and "hangar-state" in item["params"]["lista"]
    assert "lib" in item["params"]["lista"]
    h.consertar(item["conserto"])
    assert h._extensoes("pi")["ok"] is True
    assert (raiz / "extensions" / "claude-todo.ts").read_text() == "meu"
    # A pasta de helpers vai como link de diretório: sem ela, claude-bridge e git-checkpoint
    # não resolvem o import relativo e o Pi recusa as duas na largada.
    assert (raiz / "extensions" / "lib").resolve() == repo / "scripts" / "pi" / "lib"
    (raiz / "extensions" / "lib").unlink()
    assert h._extensoes("pi")["params"]["lista"] == "lib"


def test_extensao_apontando_pra_outra_fonte_nao_e_falta(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    (repo / "scripts" / "pi" / "lib").mkdir(parents=True)
    for nome in h._EXTENSOES_PI:
        (repo / "scripts" / "pi" / f"{nome}.ts").write_text("")
    monkeypatch.setattr(h, "_REPO", repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    raiz = tmp_path / ".pi" / "agent"
    ext = raiz / "extensions"
    ext.mkdir(parents=True)
    velho = tmp_path / "repo-velho" / "fullscreen-tui.ts"
    velho.parent.mkdir()
    velho.write_text("antigo")
    for nome in h._EXTENSOES_PI:
        if nome not in ("fullscreen-tui", "claude-todo"):
            (ext / f"{nome}.ts").symlink_to(repo / "scripts" / "pi" / f"{nome}.ts")
    (ext / "lib").symlink_to(repo / "scripts" / "pi" / "lib", target_is_directory=True)
    (ext / "fullscreen-tui.ts").symlink_to(velho)
    item = h._extensoes("pi")
    assert item["ok"] is False and item["codigo"] == "extensoes_outra_fonte"
    assert item["params"]["faltam"] == "claude-todo"
    h.consertar(item["conserto"])
    assert h._extensoes("pi")["ok"] is True


def test_omp_preserva_todo_e_rolagem_nativos_sem_perder_complementos(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    fontes = repo / "scripts" / "pi"
    (fontes / "lib").mkdir(parents=True)
    for nome in h._EXTENSOES_PI:
        (fontes / f"{nome}.ts").write_text("extensão")
    monkeypatch.setattr(h, "_REPO", repo)
    raiz = tmp_path / "omp-agent"
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(raiz))
    ext = raiz / "extensions"
    ext.mkdir(parents=True)
    for nome in ("claude-todo", "fullscreen-tui"):
        (ext / f"{nome}.ts").symlink_to(fontes / f"{nome}.ts")
    preferencias = raiz / "config.yml"
    preferencias.write_text("theme:\n  dark: titanium\n")
    fullscreen = raiz / "fullscreen-tui.json"
    fullscreen.write_text('{"enabled": true, "preferencia": "do usuário"}')

    h.consertar("extensoes:omp")

    assert not (ext / "claude-todo.ts").exists()
    assert not (ext / "fullscreen-tui.ts").exists()
    for nome in ("hangar-state", "rich-status-line", "claude-bridge",
                 "claude-hooks-adapter", "git-checkpoint"):
        assert (ext / f"{nome}.ts").resolve() == fontes / f"{nome}.ts"
    assert h._extensoes("omp")["ok"] is True
    assert preferencias.read_text() == "theme:\n  dark: titanium\n"
    assert json.loads(fullscreen.read_text()) == {"enabled": True, "preferencia": "do usuário"}

    # Arquivos e links personalizados não pertencem ao instalador.
    (ext / "claude-todo.ts").write_text("todo personalizado")
    outra_fonte = tmp_path / "fullscreen-personalizado.ts"
    outra_fonte.write_text("fullscreen personalizado")
    (ext / "fullscreen-tui.ts").symlink_to(outra_fonte)
    h.consertar("extensoes:omp")
    assert (ext / "claude-todo.ts").read_text() == "todo personalizado"
    assert (ext / "fullscreen-tui.ts").resolve() == outra_fonte

    (ext / "claude-hooks-adapter.ts").unlink()
    assert h._extensoes("omp")["ok"] is False
    assert "claude-hooks-adapter" in h._extensoes("omp")["params"]["lista"]

def test_hooks_do_claude_faltando_apontam_o_conserto(tmp_path):
    (tmp_path / "settings.json").write_text(json.dumps({"hooks": {"Stop": [{"hooks": [{"command": "x state_hook.py"}]}]}}))
    item = h._hooks_claude(tmp_path)
    assert item["ok"] is False and item["conserto"] == "hooks-claude"
    assert "askq_capture.py" in item["params"]["lista"] and "state_hook.py" not in item["params"]["lista"]
    (tmp_path / "settings.json").write_text("{quebrado")
    assert h._hooks_claude(tmp_path)["ok"] is None


def test_codex_nao_oferece_a_ponte_antiga_no_painel(tmp_path, monkeypatch):
    (tmp_path / ".codex").mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(h, "list_config_dirs", lambda: [])
    monkeypatch.setattr(h, "_versao", lambda cli: "0.153.4" if cli == "codex" else None)
    codex = next(item for item in h.diagnosticar() if item["id"] == "codex")
    assert codex["instalado"] is True
    assert {item["id"] for item in codex["itens"]} == {"credenciais", "wrapper", "hooks", "mcp", "modelo"}
    assert all(item["codigo"] != "sem_ponte" and item["conserto"] != "skills" for item in codex["itens"])


def test_omp_nao_oferece_fullscreen(tmp_path, monkeypatch):
    # A conversa do omp mora no scrollback do terminal por desenho (renderizador nunca consulta a
    # posicao de rolagem; issue #10232). Em alternate screen ela some, e a roda vira seta =
    # historico no composer. A tela de saude nao pode oferecer o botao que liga isso.
    raiz = tmp_path / "omp-agent"
    (raiz / "extensions").mkdir(parents=True)
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(raiz))
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))   # credenciais/cofre
    monkeypatch.setattr(h, "_versao", lambda cli: "18.1.6" if cli == "omp" else None)
    omp = next(b for b in h.diagnosticar() if b["id"] == "omp")
    assert "fullscreen" not in [i["id"] for i in omp["itens"]]
    try:
        h.consertar("fullscreen:omp")
    except ValueError:
        return
    raise AssertionError("fullscreen:omp não pode ser conserto — liga o que quebra a rolagem")


def test_conserto_desconhecido_ou_caminho_arbitrario_falha_alto():
    for id_ in ("nada", "extensoes:/tmp/qualquer", "sync:/tmp/qualquer", "fullscreen:../../etc", "fullscreen:kimi"):
        try:
            h.consertar(id_)
        except ValueError:
            continue
        raise AssertionError(f"{id_} não pode virar no-op nem escrever fora dos agentes")


def test_chave_no_omp_nao_duplica(tmp_path, monkeypatch):
    import sqlite3
    from app import oauth_codex
    db = tmp_path / "agent.db"
    con = sqlite3.connect(db)
    con.execute("create table auth_credentials (id integer primary key autoincrement, provider text not null, "
                "credential_type text not null, data text not null, disabled_cause text, identity_key text)")
    con.commit(); con.close()
    monkeypatch.setattr(oauth_codex, "_omp_db", lambda home=None: db)
    assert h._omp_gravar_chave("opencode", "k1") == (True, str(db))
    assert h._omp_gravar_chave("opencode", "k2") == (True, "ja-existe")
    con = sqlite3.connect(db)
    assert con.execute("select count(*) from auth_credentials").fetchone()[0] == 1


def test_wrapper_por_shell_que_a_pessoa_tem(tmp_path, monkeypatch):
    """Um CLI instalado à mão ficava todo verde sem esta linha, e nada que ele abrisse aparecia no app."""
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    monkeypatch.setattr(h, "_E_WINDOWS", False)          # cenário POSIX: fish e bash
    monkeypatch.setattr(h.shutil, "which", lambda nome: "/bin/bash" if nome == "bash" else None)
    (tmp_path / ".config" / "fish" / "functions").mkdir(parents=True)
    # O caminho é de OUTRO clone de propósito: wrapper apontando pra outro checkout funciona igual,
    # e chamá-lo de ausente seria mentira.
    (tmp_path / ".bashrc").write_text('source "/qualquer/clone/scripts/shell/pi.posix.sh"\n')

    item = h._wrapper("pi")
    assert item["ok"] is False and item["codigo"] == "wrapper_falta"
    assert item["params"]["lista"] == "fish" and item["conserto"] == "wrapper"

    (tmp_path / ".config" / "fish" / "functions" / "pi.fish").write_text("function pi\nend\n")
    item = h._wrapper("pi")
    assert item["ok"] is True and item["params"]["onde"] == "fish, bash"

    # O Codex depende também dos lançadores: é por eles que o BACKEND abre a sessão.
    (tmp_path / ".config" / "fish" / "functions" / "codex.fish").write_text("function codex\nend\n")
    (tmp_path / ".bashrc").write_text('source "/qualquer/clone/scripts/shell/codex.posix.sh"\n')
    # Rotulado: sem isso a frase "falta em: fish, hangar-codex" faz o lançador parecer um shell.
    assert h._wrapper("codex")["params"]["lista"] == "lançador hangar-codex, lançador hangar-codex-tui"
    (tmp_path / ".local" / "bin").mkdir(parents=True)
    for nome in ("hangar-codex", "hangar-codex-tui"):
        (tmp_path / ".local" / "bin" / nome).write_text("")
    assert h._wrapper("codex")["ok"] is True


def test_wrapper_windows_le_o_perfil_que_o_instalador_escreve(tmp_path, monkeypatch):
    # O instalador grava o bloco no `profile.ps1` (CurrentUserAllHosts) do 5.1, com contrabarra.
    # A checagem olhava so o `Microsoft.PowerShell_profile.ps1` do 7 e procurava barra normal: o
    # Codex instalado pelo app aparecia "nenhum rc de shell" com o wrapper carregado (12/09/2026).
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    monkeypatch.setattr(h, "_E_WINDOWS", True)
    perfil = tmp_path / "Documents" / "WindowsPowerShell" / "profile.ps1"
    perfil.parent.mkdir(parents=True)
    perfil.write_text('# >>> hangar >>>\n. "C:\\Users\\x\\hangar\\scripts\\shell\\codex.ps1"\n',
                      encoding="utf-8-sig")
    (tmp_path / ".local" / "bin").mkdir(parents=True)
    for nome in ("hangar-codex", "hangar-codex-tui"):
        (tmp_path / ".local" / "bin" / nome).write_text("")
    item = h._wrapper("codex")
    assert item["ok"] is True and item["params"]["onde"] == "PowerShell"
    # Perfil presente sem o bloco daquele CLI e falta de verdade, com o conserto do Windows.
    item = h._wrapper("pi")
    assert item["codigo"] == "wrapper_falta" and item["conserto"] == "wrapper"


def test_conserto_wrapper_no_windows_usa_o_script_do_perfil(monkeypatch):
    # Sem isto o wrapper faltando no Windows nao tinha botao: o unico conserto era o bash.
    monkeypatch.setattr(h, "_E_WINDOWS", True)
    monkeypatch.setattr(h.shutil, "which", lambda cli: "/resolved/powershell.exe" if cli == "powershell.exe" else None)
    chamado = {}

    def _run(argv, **kw):
        chamado["argv"] = argv
        return h.subprocess.CompletedProcess(argv, 0, "ok", "")

    monkeypatch.setattr(h.subprocess, "run", _run)
    h.consertar("wrapper")
    assert chamado["argv"][0] == "/resolved/powershell.exe"
    assert chamado["argv"][-2:] == [str(h._REPO / "scripts" / "setup-windows-wrappers.ps1"), "-Apply"]


def _tmux_windows(tmp_path, monkeypatch, titulos="off"):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.delenv("PSMUX_CONFIG_FILE", raising=False)
    monkeypatch.setattr(h, "_E_WINDOWS", True)
    monkeypatch.setattr(h, "_versao", lambda cli: "tmux 3.3.8")
    respostas = {"default-terminal": "xterm-256color", "set-titles-string": titulos, "mouse": "on"}
    monkeypatch.setattr(h, "_tmux", lambda args: (
        "COLORTERM=truecolor\nCLAUDE_CODE_TMUX_TRUECOLOR=1" if args[0] == "show-environment"
        else respostas.get(args[-1], "")))


def test_tmux_windows_le_o_bloco_do_psmux_e_nao_exige_titulo(tmp_path, monkeypatch):
    # No Windows o bloco e o do setup-windows-tmux.ps1 (outro marcador) e os titulos ficam
    # desligados de proposito. O card acusava os dois e o Consertar chamava o instalador bash.
    _tmux_windows(tmp_path, monkeypatch)
    (tmp_path / ".tmux.conf").write_text("# >>> hangar windows-tmux >>>\nset -g set-titles off\n"
                                         "# <<< hangar windows-tmux <<<\n")
    itens = {i["id"]: i for i in h._card_tmux()["itens"]}
    assert itens["bloco"]["ok"] is True
    assert "titulo" not in itens


def test_tmux_windows_sem_bloco_segue_a_precedencia_do_psmux(tmp_path, monkeypatch):
    # O psmux le o PRIMEIRO arquivo que existe e para: bloco no .tmux.conf com .psmux.conf
    # presente e config morta.
    _tmux_windows(tmp_path, monkeypatch)
    (tmp_path / ".psmux.conf").write_text("set -g mouse on\n")
    (tmp_path / ".tmux.conf").write_text("# >>> hangar windows-tmux >>>\n")
    item = next(i for i in h._card_tmux()["itens"] if i["id"] == "bloco")
    assert item["ok"] is False and item["conserto"] == "tmux"


def test_conserto_tmux_no_windows_usa_o_script_do_psmux(monkeypatch):
    monkeypatch.setattr(h, "_E_WINDOWS", True)
    monkeypatch.setattr(h.shutil, "which", lambda cli: "/resolved/powershell.exe" if cli == "powershell.exe" else None)
    chamado = {}

    def _run(argv, **kw):
        chamado["argv"] = argv
        return h.subprocess.CompletedProcess(argv, 0, "ok", "")

    monkeypatch.setattr(h.subprocess, "run", _run)
    h.consertar("tmux")
    assert chamado["argv"][0] == "/resolved/powershell.exe"
    assert chamado["argv"][-3:] == [str(h._REPO / "scripts" / "setup-windows-tmux.ps1"), "-Apply", "-SkipInstall"]


def test_wrapper_sem_rc_nenhum_nao_acusa_falta(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    item = h._wrapper("pi")
    assert item["ok"] is None and item["codigo"] == "wrapper_sem_shell"


@pytest.mark.parametrize("mode", ["rust", "pending"])
def test_oauth_health_repair_cannot_restore_python_codex_writer(tmp_path, monkeypatch, mode):
    import json
    from pathlib import Path
    from types import SimpleNamespace
    from fastapi import HTTPException
    from app import account_bridge, runtime_coordinator, oauth_codex
    from app import harness_saude
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / ".codex"))
    (tmp_path / ".codex").mkdir()
    vault = tmp_path / ".hangar/auth/openai-codex.json"
    vault.parent.mkdir(parents=True)
    vault.write_text(json.dumps({"access": "synthetic", "refresh": "synthetic", "id_token": "",
                                 "expires_ms": 1, "account_id": "fixture", "plano": ""}))
    monkeypatch.setattr(account_bridge, "_preparation_transport", None)
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode=mode))
    with pytest.raises(HTTPException) as issue:
        harness_saude.consertar("oauth")
    assert issue.value.status_code == 503
    assert not (tmp_path / ".codex/auth.json").exists()
