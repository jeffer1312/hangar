import tomllib

from app.kimi_hook_installer import ensure_kimi_hooks_installed, _ENTRIES, HOOK


def _cfg(tmp_path):
    return tmp_path / "config.toml"


def test_installs_all_events_and_preserves_user_content(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    _cfg(tmp_path).write_text('default_model = "apikey/k3"\n\n[providers.x]\nbase_url = "y"\n',
                              encoding="utf-8")
    touched = ensure_kimi_hooks_installed()
    assert touched == [str(_cfg(tmp_path))]
    raw = _cfg(tmp_path).read_text(encoding="utf-8")
    # Conteudo do usuario INTACTO (o append nao reescreve nada).
    assert raw.startswith('default_model = "apikey/k3"')
    data = tomllib.loads(raw)  # o resultado TEM que continuar TOML valido
    got = [(h["event"], h.get("matcher")) for h in data["hooks"] if HOOK in h.get("command", "")]
    assert sorted(got) == sorted(_ENTRIES)
    assert data["providers"]["x"]["base_url"] == "y"
    # Backup da primeira mexida.
    assert (tmp_path / "config.toml.bak-hangar").is_file()


def test_idempotent_second_run_changes_nothing(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    ensure_kimi_hooks_installed()
    first = _cfg(tmp_path).read_text(encoding="utf-8")
    assert ensure_kimi_hooks_installed() == []  # ja instalado -> skip
    assert _cfg(tmp_path).read_text(encoding="utf-8") == first


def test_partial_install_only_appends_what_is_missing(tmp_path, monkeypatch):
    # Idempotencia POR ENTRADA: um config que ja tem SO UMA entrada nossa (ex: instalada a mao,
    # ou de uma versao antiga do installer) recebe apenas as que faltam — sem duplicar a existente.
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    from app.kimi_hook_installer import _blocks
    _cfg(tmp_path).write_text(_blocks([("Stop", None)]).lstrip("\n"), encoding="utf-8")
    ensure_kimi_hooks_installed()
    data = tomllib.loads(_cfg(tmp_path).read_text(encoding="utf-8"))
    got = [(h["event"], h.get("matcher")) for h in data["hooks"] if HOOK in h.get("command", "")]
    assert sorted(got) == sorted(_ENTRIES)
    assert got.count(("Stop", None)) == 1  # nao duplicou


def test_creates_config_when_missing(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    assert ensure_kimi_hooks_installed() == [str(_cfg(tmp_path))]
    data = tomllib.loads(_cfg(tmp_path).read_text(encoding="utf-8"))
    assert any(HOOK in h.get("command", "") for h in data["hooks"])


def test_broken_config_is_never_touched(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    _cfg(tmp_path).write_text("isto nao = [ toml valido", encoding="utf-8")
    assert ensure_kimi_hooks_installed() == []
    assert _cfg(tmp_path).read_text(encoding="utf-8") == "isto nao = [ toml valido"


def test_inline_hooks_array_is_skipped(tmp_path, monkeypatch):
    # hooks = [...] inline (fora da doc): apendar [[hooks]] seria redefinicao e quebraria o TOML.
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    raw = 'hooks = [{event = "Stop", command = "x"}]\n'
    _cfg(tmp_path).write_text(raw, encoding="utf-8")
    assert ensure_kimi_hooks_installed() == []
    assert _cfg(tmp_path).read_text(encoding="utf-8") == raw


def test_no_kimi_home_noop(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path / "nao-existe"))
    assert ensure_kimi_hooks_installed() == []


def test_foreign_hooks_are_kept(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    _cfg(tmp_path).write_text('[[hooks]]\nevent = "Stop"\ncommand = "terminal-notifier -m x"\n',
                              encoding="utf-8")
    ensure_kimi_hooks_installed()
    data = tomllib.loads(_cfg(tmp_path).read_text(encoding="utf-8"))
    commands = [h["command"] for h in data["hooks"]]
    assert "terminal-notifier -m x" in commands
    assert any(HOOK in c for c in commands)
