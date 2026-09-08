"""Painel de saúde dos harnesses (app/harness_saude.py): checagem lê, conserto reusa o instalador."""
import json
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


def test_wrapper_sem_rc_nenhum_nao_acusa_falta(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    item = h._wrapper("pi")
    assert item["ok"] is None and item["codigo"] == "wrapper_sem_shell"
