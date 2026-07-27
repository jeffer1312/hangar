import json
import os

from app import registry


def _pane(monkeypatch, cmd: str):
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: cmd)


def test_detects_pi_from_the_process_command(monkeypatch):
    # Deteccao pelo executavel REAL. Casar por nome de sessao ou por cwd marcaria como Pi
    # qualquer sessao aberta na mesma pasta.
    _pane(monkeypatch, "pi --session-id abc")
    assert registry.provider_of_pane(99) == "pi"
    _pane(monkeypatch, "/usr/bin/pi -e x.ts --session-id abc")
    assert registry.provider_of_pane(99) == "pi"


def test_detects_pi_with_the_argv_rewritten(monkeypatch):
    # Como o pane REAL aparece (Task 0, fato 7): o pi sobrescreve o proprio argv e o /proc devolve
    # "pi" seguido de NUL (que _cmdline vira espaco). Sem o strip/split, argv0 nunca casa.
    _pane(monkeypatch, "pi" + " " * 120)
    assert registry.provider_of_pane(99) == "pi"


def test_does_not_match_a_command_merely_containing_pi(monkeypatch):
    # `pip install`, `pipx`, `mpirun` e um caminho com /pi/ dentro nao sao o agente Pi.
    for cmd in ("pip install requests", "pipx run x", "python -m pip list",
                "mpirun -n 2 a.out", "/opt/pi/bin/node server.js"):
        _pane(monkeypatch, cmd)
        assert registry.provider_of_pane(99) != "pi", cmd


def test_provider_of_pane_reads_proc_not_a_pane_field(monkeypatch):
    # list_panes_active() NAO devolve o comando (tmux.py:106-122): ele vem de /proc, como
    # _repl_sid ja faz. Este teste trava esse contrato — se alguem "simplificar" pra ler um campo
    # inexistente do pane, todo pane vira "claude" calado.
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11, 12])
    monkeypatch.setattr(registry, "_cmdline",
                        lambda p: {11: "fish", 12: "pi --session-id x"}.get(p, ""))
    assert registry.provider_of_pane(99) == "pi"

    monkeypatch.setattr(registry, "_cmdline",
                        lambda p: {11: "fish", 12: "claude --session-id x"}.get(p, ""))
    assert registry.provider_of_pane(99) == "claude"


def test_provider_of_pane_defaults_to_claude_when_nothing_matches(monkeypatch):
    # Default preserva o comportamento de hoje: qualquer pane nao reconhecido segue tratado como
    # Claude, como antes desta task existir.
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: "vim")
    assert registry.provider_of_pane(99) == "claude"


def test_session_file_comes_from_the_pane_sidecar(monkeypatch, tmp_path):
    # O bilhete da extensao carrega o caminho EXATO. E o unico sinal que existe pra um `pi` digitado
    # na mao: o argv nao tem o id (Task 0, fato 7) e sem wrapper nao ha CP_PI_SESSION.
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    alvo = tmp_path / "2026_x.jsonl"
    alvo.write_text("")
    (cfg / ".claude-pocket-pi" / "123.json").write_text(json.dumps({"file": str(alvo)}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    assert registry.pi_session_file("%123", pid=7) == str(alvo)


def test_session_file_ignores_a_stale_sidecar(monkeypatch, tmp_path):
    # cp-state.ts NUNCA apaga o bilhete quando o pane fecha, e o tmux reusa %pane_id apos um restart
    # do servidor (ex: reboot) -> um bilhete ORFAO apontando pra um .jsonl ja deletado/renomeado nao
    # pode ser devolvido como se fosse o transcript deste pane (a MESMA classe de bug que o Step 6
    # ja cobre pro fallback newest-by-mtime, so que chegando por um bilhete velho em vez do mtime).
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    (cfg / ".claude-pocket-pi" / "123.json").write_text(
        json.dumps({"file": str(tmp_path / "sumiu.jsonl")}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    # env tambem sem nada, senao o fallback mascararia o bilhete ignorado com um resultado valido.
    monkeypatch.setattr(registry, "_proc_environ_path", lambda pid: str(tmp_path / "nao-existe"))
    assert registry.pi_session_file("%123", pid=7, cwd="/w") is None


def _fake_proc_start(monkeypatch, tmp_path, nasceu: float):
    # /proc/<pid>/stat de mentira com o processo nascendo em `nasceu` (epoch). O comm leva espaco E
    # parentese de proposito: e o caso que quebra qualquer contagem de campo feita da esquerda.
    with open("/proc/stat") as fh:
        btime = next(float(l.split()[1]) for l in fh if l.startswith("btime "))
    ticks = (nasceu - btime) * os.sysconf("SC_CLK_TCK")
    stat = tmp_path / "stat"
    stat.write_text(f"7 (pi (fork) x) S " + "0 " * 18 + f"{ticks:.0f} 0 0\n")
    monkeypatch.setattr(registry, "_proc_stat_path", lambda pid: str(stat))


def test_proc_start_time_survives_a_comm_with_spaces_and_parens(monkeypatch, tmp_path):
    # Sem o rindex(")") o campo 22 sai errado por alguns tokens e o frescor do bilhete vira ruido.
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)
    assert abs(registry._proc_start_time(7) - 1_700_000_000.0) < 1
    monkeypatch.setattr(registry, "_proc_stat_path", lambda pid: str(tmp_path / "nao-existe"))
    assert registry._proc_start_time(7) is None      # degrada como os vizinhos de /proc


def test_session_file_rejects_a_sidecar_older_than_the_pane_process(monkeypatch, tmp_path):
    # Caso que o guarda original existia pra pegar: apos um restart do servidor tmux o %pane_id e
    # reusado e o .jsonl da sessao ANTERIOR continua no disco, entao o exists() deixa passar. O
    # bilhete foi escrito ANTES de o processo deste pane nascer -> e de outra encarnacao, cai no env.
    # id null e o caso que NENHUM guarda por divergencia de id pegava (publishPane escreve
    # `getSessionId() ?? null`) e o mais comum num bilhete velho.
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    velho = tmp_path / "2026-07-01T00-00-00-000Z_aaa.jsonl"
    velho.write_text("")
    (cfg / ".claude-pocket-pi" / "9.json").write_text(
        json.dumps({"file": str(velho), "id": None, "ts": 1_700_000_000.0}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_600.0)     # pane nasceu 10min DEPOIS
    env = tmp_path / "environ"
    env.write_bytes(b"CP_PI_SESSION=bbb\x00")
    monkeypatch.setattr(registry, "_proc_environ_path", lambda pid: str(env))
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: f"/s/2026_{s}.jsonl")

    assert registry.pi_session_file("%9", pid=7, cwd="/w") == "/s/2026_bbb.jsonl"


def test_session_file_trusts_a_fresh_sidecar_even_with_another_id(monkeypatch, tmp_path):
    # Pos-/fork (ou /tree, ou troca de sessao): a extensao reescreve o bilhete no agent_start com a
    # sessao NOVA, e o CP_PI_SESSION segue congelado na original desde o exec. Divergir e o correto
    # — quem manda e o bilhete, que foi escrito depois de o processo nascer.
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    alvo = tmp_path / "2026-07-27T00-00-00-000Z_bbb.jsonl"
    alvo.write_text("")
    (cfg / ".claude-pocket-pi" / "9.json").write_text(
        json.dumps({"file": str(alvo), "id": "bbb", "ts": 1_700_000_600.0}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)     # pane nasceu ANTES do bilhete
    env = tmp_path / "environ"
    env.write_bytes(b"CP_PI_SESSION=aaa\x00")
    monkeypatch.setattr(registry, "_proc_environ_path", lambda pid: str(env))
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: f"/s/2026_{s}.jsonl")
    assert registry.pi_session_file("%9", pid=7, cwd="/w") == str(alvo)


def test_session_file_falls_back_to_the_wrapper_env(monkeypatch, tmp_path):
    # Sem bilhete (extensao nao carregou) o wrapper ainda salva: CP_PI_SESSION no /proc/<pid>/environ,
    # mesmo truque do _engine_of. O id vira caminho pelo adapter, que resolve o glob <ts>_<uuid>.
    cfg = tmp_path / "cfg"
    cfg.mkdir()
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    env = tmp_path / "environ"
    sid = "019fa3d5-f074-707b-92a8-1ca7f1d99ec9"
    env.write_bytes(b"PATH=/bin\x00CP_PI_SESSION=" + sid.encode() + b"\x00")
    monkeypatch.setattr(registry, "_proc_environ_path", lambda pid: str(env))
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: f"/s/2026_{s}.jsonl")
    assert registry.pi_session_file("%123", pid=7, cwd="/w") == f"/s/2026_{sid}.jsonl"


def test_session_file_is_none_when_nothing_knows(monkeypatch, tmp_path):
    # Nem bilhete nem env: a sessao ainda aparece na lista, so sem transcript — nunca chutar o
    # arquivo de outro agente (a regressao do Step 6).
    cfg = tmp_path / "cfg"
    cfg.mkdir()
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    monkeypatch.setattr(registry, "_proc_environ_path", lambda pid: str(tmp_path / "nao-existe"))
    assert registry.pi_session_file("%123", pid=7, cwd="/w") is None


def test_pi_pane_does_not_inherit_a_claude_transcript(monkeypatch, tmp_path):
    # Regressao mais cara desta task: sem o ramo do Pi, resolve_jsonl devolve o transcript do
    # Claude do mesmo cwd e a sessao Pi abre mostrando a conversa de OUTRO agente.
    projetos = tmp_path / "claude-projects"
    slug = projetos / "-w"
    slug.mkdir(parents=True)
    (slug / "11111111-1111-1111-1111-111111111111.jsonl").write_text("")
    monkeypatch.setattr(registry.settings, "projects_dir", str(projetos))

    monkeypatch.setattr(registry.tmux, "list_panes_active",
                        lambda: [{"name": "s-pi", "pid": 99, "cwd": "/w", "pane_id": "%9"}])
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: "pi" + " " * 80)
    monkeypatch.setattr(registry, "pi_session_file", lambda *a, **k: None)

    infos = registry.SessionRegistry().list()
    pi = [i for i in infos if i.name == "s-pi"]
    assert len(pi) == 1, "sessao Pi nao pode aparecer duplicada"
    assert pi[0].provider == "pi"
    assert "claude-projects" not in (pi[0].jsonl or ""), "Pi herdou o transcript do Claude"


def test_pi_pane_without_a_known_transcript_stays_tracked(monkeypatch, tmp_path):
    # jsonl=None pra um pane Pi e "ainda sem 1o turno", nao um chute ambiguo (a identidade do pane
    # e deterministica: bilhete/env, nunca newest-by-mtime) -> tracked continua True, senao a UI
    # desliga o card (SessionCard.svelte: untracked) e a sessao recem-criada fica inclicavel antes
    # do 1o turno. Mesmo precedente do Codex (sempre tracked=True, independente do rollout_path).
    projetos = tmp_path / "claude-projects"
    monkeypatch.setattr(registry.settings, "projects_dir", str(projetos))

    monkeypatch.setattr(registry.tmux, "list_panes_active",
                        lambda: [{"name": "s-pi", "pid": 99, "cwd": "/w", "pane_id": "%9"}])
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: "pi" + " " * 80)
    monkeypatch.setattr(registry, "pi_session_file", lambda *a, **k: None)

    infos = registry.SessionRegistry().list()
    pi = [i for i in infos if i.name == "s-pi"]
    assert len(pi) == 1
    assert pi[0].jsonl is None
    assert pi[0].tracked is True
