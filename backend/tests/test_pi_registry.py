import json
import os

import pytest

from app import registry
from app import procinfo


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
    # `ts` posterior ao nascimento do pane: o bilhete so vale quando da pra PROVAR que e desta
    # encarnacao do pane (ver test_session_file_refuses_a_sidecar_when_the_pane_birth_is_unknown).
    (cfg / ".claude-pocket-pi" / "123.json").write_text(
        json.dumps({"file": str(alvo), "ts": 1_700_000_600.0}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)
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
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(tmp_path / "nao-existe"))
    assert registry.pi_session_file("%123", pid=7, cwd="/w") is None


def _fake_proc_start(monkeypatch, tmp_path, nasceu: float):
    # /proc/<pid>/stat de mentira com o processo nascendo em `nasceu` (epoch). O comm leva espaco E
    # parentese de proposito: e o caso que quebra qualquer contagem de campo feita da esquerda.
    with open("/proc/stat") as fh:
        btime = next(float(l.split()[1]) for l in fh if l.startswith("btime "))
    ticks = (nasceu - btime) * os.sysconf("SC_CLK_TCK")
    stat = tmp_path / "stat"
    stat.write_text(f"7 (pi (fork) x) S " + "0 " * 18 + f"{ticks:.0f} 0 0\n")
    monkeypatch.setattr(procinfo, "_proc_stat_path", lambda pid: str(stat))


def test_proc_start_time_survives_a_comm_with_spaces_and_parens(monkeypatch, tmp_path):
    # Sem o rindex(")") o campo 22 sai errado por alguns tokens e o frescor do bilhete vira ruido.
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)
    assert abs(registry._proc_start_time(7) - 1_700_000_000.0) < 1
    monkeypatch.setattr(procinfo, "_proc_stat_path", lambda pid: str(tmp_path / "nao-existe"))
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
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(env))
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
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(env))
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: f"/s/2026_{s}.jsonl")
    assert registry.pi_session_file("%9", pid=7, cwd="/w") == str(alvo)


def test_session_file_refuses_a_sidecar_pointing_at_a_subagent_run(monkeypatch, tmp_path):
    # O Pi dispara agent_start TAMBEM pro subagente (Task tool), e o publishPane da extensao
    # reescreve o bilhete com o transcript DELE. Aceitar trocava a conversa inteira da sessao pela
    # do subagente no app — medido 2026-07-30 no pane %26 (sessao real), enquanto o terminal
    # seguia normal. O bilhete e FRESCO aqui de proposito: o guarda de frescor nao pega este caso.
    registry._PI_TICKET_WARNED.clear()
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    raiz = tmp_path / "2026-07-30T20-29-24-651Z_18e48e08.jsonl"
    raiz.write_text("")
    run = tmp_path / "2026-07-30T20-29-24-651Z_18e48e08" / "44bad0fb" / "run-2"
    run.mkdir(parents=True)
    (run / "session.jsonl").write_text("")
    (cfg / ".claude-pocket-pi" / "9.json").write_text(
        json.dumps({"file": str(run / "session.jsonl"), "id": "sub", "ts": 1_700_000_600.0}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)      # pane nasceu ANTES do bilhete
    env = tmp_path / "environ"
    env.write_bytes(b"CP_PI_SESSION=18e48e08\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(env))
    # Este fallback NAO pode ser o que salva: com cwd cheio de espaco/acento ele ja falhou de
    # verdade. Quem devolve a conversa e o proprio caminho do subagente, que carrega a raiz.
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: "")

    # Volta pra sessao RAIZ (a conversa que o usuario ve no terminal), nunca None.
    assert registry.pi_session_file("%9", pid=7, cwd="/w") == str(raiz)


def _bilhete_e_env(monkeypatch, tmp_path, dados: dict):
    # Bilhete apontando pra um .jsonl que EXISTE (senao o exists() rejeitaria por outro motivo) +
    # CP_PI_SESSION apontando pra outra sessao: e o env que tem que ganhar quando o frescor do
    # bilhete nao da pra estabelecer.
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    velho = tmp_path / "2026-07-01T00-00-00-000Z_aaa.jsonl"
    velho.write_text("")
    (cfg / ".claude-pocket-pi" / "9.json").write_text(json.dumps({"file": str(velho), **dados}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    env = tmp_path / "environ"
    env.write_bytes(b"CP_PI_SESSION=bbb\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(env))
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: f"/s/2026_{s}.jsonl")
    return velho


def test_session_file_refuses_a_sidecar_when_the_pane_birth_is_unknown(monkeypatch, tmp_path, caplog):
    # /proc/<pid>/stat ilegivel (pid morto, permissao, kernel sem /proc) -> _proc_start_time devolve
    # None e o frescor do bilhete e INDETERMINAVEL. Antes o guarda simplesmente nao rodava e o
    # bilhete era aceito — exatamente a falha que ele existe pra impedir: pane_id reusado apos
    # restart do tmux, .jsonl da sessao anterior ainda no disco (o exists() nao salva), e o pane
    # novo abrindo a conversa VELHA, tracked=True, sem log nenhum.
    registry._PI_TICKET_WARNED.clear()
    _bilhete_e_env(monkeypatch, tmp_path, {"id": None, "ts": 1_700_000_000.0})
    monkeypatch.setattr(procinfo, "_proc_stat_path", lambda pid: str(tmp_path / "nao-existe"))
    assert registry._proc_start_time(7) is None

    with caplog.at_level("WARNING", logger="claude_pocket.registry"):
        assert registry.pi_session_file("%9", pid=7, cwd="/w") == "/s/2026_bbb.jsonl"
        assert registry.pi_session_file("%9", pid=7, cwd="/w") == "/s/2026_bbb.jsonl"
    # Uma vez, nao a cada poll: list() roda de segundo em segundo e um warning por varredura
    # entupiria o journal ate ninguem mais ler nenhum.
    assert len([r for r in caplog.records if "bilhete" in r.getMessage()]) == 1


def test_session_file_refuses_a_sidecar_without_a_timestamp(monkeypatch, tmp_path):
    # Bilhete sem `ts` numerico (build antiga da extensao, escrita parcial): mesma indeterminacao,
    # mesma decisao — cai no env em vez de confiar num bilhete que pode ser de outra encarnacao.
    registry._PI_TICKET_WARNED.clear()
    _bilhete_e_env(monkeypatch, tmp_path, {"id": "aaa"})
    _fake_proc_start(monkeypatch, tmp_path, 1_700_000_000.0)

    assert registry.pi_session_file("%9", pid=7, cwd="/w") == "/s/2026_bbb.jsonl"


def test_session_file_falls_back_to_the_wrapper_env(monkeypatch, tmp_path):
    # Sem bilhete (extensao nao carregou) o wrapper ainda salva: CP_PI_SESSION no /proc/<pid>/environ,
    # mesmo truque do _engine_of. O id vira caminho pelo adapter, que resolve o glob <ts>_<uuid>.
    cfg = tmp_path / "cfg"
    cfg.mkdir()
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    env = tmp_path / "environ"
    sid = "019fa3d5-f074-707b-92a8-1ca7f1d99ec9"
    env.write_bytes(b"PATH=/bin\x00CP_PI_SESSION=" + sid.encode() + b"\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(env))
    monkeypatch.setattr(registry, "_pi_transcript_of_id", lambda cwd, s: f"/s/2026_{s}.jsonl")
    assert registry.pi_session_file("%123", pid=7, cwd="/w") == f"/s/2026_{sid}.jsonl"


def test_session_file_is_none_when_nothing_knows(monkeypatch, tmp_path):
    # Nem bilhete nem env: a sessao ainda aparece na lista, so sem transcript — nunca chutar o
    # arquivo de outro agente (a regressao do Step 6).
    cfg = tmp_path / "cfg"
    cfg.mkdir()
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(tmp_path / "nao-existe"))
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


def _pane_pi_sem_transcript(monkeypatch, tmp_path):
    projetos = tmp_path / "claude-projects"
    monkeypatch.setattr(registry.settings, "projects_dir", str(projetos))
    monkeypatch.setattr(registry.tmux, "list_panes_active",
                        lambda: [{"name": "s-pi", "pid": 99, "cwd": "/w", "pane_id": "%9"}])
    monkeypatch.setattr(registry, "_descendant_pids", lambda pid, children=None: [11])
    monkeypatch.setattr(registry, "_cmdline", lambda p: "pi" + " " * 80)
    monkeypatch.setattr(registry, "pi_session_file", lambda *a, **k: None)


def test_pi_pane_without_a_known_transcript_is_untracked(monkeypatch, tmp_path):
    # REVERTE o tracked=True fixo (4ac802b). O argumento de la era "senao a sessao recem-criada
    # fica inclicavel antes do 1o turno" — mas clicavel ela nunca foi util: /events (api.py:880) e
    # /history (api.py:815) exigem `info.jsonl` e devolvem 404 sem ele. tracked=True so escondia o
    # motivo: card clicavel, chat que nao carrega, e nenhuma das afordancias de "sem id" nas duas
    # views. Com False a linha diz o que houve (Sidebar/SessionCard: untrackedReason) e o usuario
    # pode matar/reabrir. Some sozinho: no 1o turno o Pi escreve o transcript e a varredura
    # seguinte devolve tracked=True.
    _pane_pi_sem_transcript(monkeypatch, tmp_path)

    infos = registry.SessionRegistry().list()
    pi = [i for i in infos if i.name == "s-pi"]
    assert len(pi) == 1
    assert pi[0].jsonl is None
    assert pi[0].tracked is False


def test_pi_pane_with_a_transcript_is_tracked(monkeypatch, tmp_path):
    # A outra metade: resolvido o transcript, a sessao volta a ser uma sessao normal (chat abre).
    _pane_pi_sem_transcript(monkeypatch, tmp_path)
    monkeypatch.setattr(registry, "pi_session_file", lambda *a, **k: "/s/2026_aaa.jsonl")

    pi = [i for i in registry.SessionRegistry().list() if i.name == "s-pi"]
    assert pi[0].jsonl == "/s/2026_aaa.jsonl"
    assert pi[0].tracked is True


def test_resume_refuses_a_pi_pane_instead_of_killing_it(monkeypatch, tmp_path):
    # O botao "Retomar conversa" so aparece numa linha untracked — que agora inclui a sessao Pi sem
    # transcript. O resume e Claude-only ponta a ponta: candidatos de ~/.claude/projects e relance
    # com `claude --resume <uuid>` DEPOIS de tmux.kill_session. Num pane Pi isso ofereceria a
    # conversa do agente ERRADO e mataria a sessao viva pra subir um claude no lugar.
    _pane_pi_sem_transcript(monkeypatch, tmp_path)
    mortes = []
    monkeypatch.setattr(registry.tmux, "kill_session", lambda n: mortes.append(n))

    reg = registry.SessionRegistry()
    for chamada in (lambda: reg.resume_candidates("s-pi"),
                    lambda: reg.resume("s-pi", "11111111-1111-1111-1111-111111111111")):
        with pytest.raises(ValueError) as exc:
            chamada()
        assert "pi" in str(exc.value)
    assert mortes == [], "o pane Pi nao pode ser morto pelo caminho de resume do Claude"


# ---------------------------------------------------------------------------
# create(provider="pi"): mesmo caminho tmux do Claude, transcript NAO pre-semeado.
# ---------------------------------------------------------------------------
_UUID_PI = "22222222-2222-2222-2222-222222222222"


def _create_pi(tmp_path, monkeypatch, **kw):
    from unittest.mock import patch
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as ns, \
         patch.object(registry, "_pretrust_cwd") as pt:
        info = reg.create("s-pi", "/home/u/p", provider="pi", **kw)
    return info, ns, pt


def test_create_pi_does_not_seed_a_claude_transcript_path(tmp_path, monkeypatch):
    # O jsonl do Pi e <ts>_<uuid>.jsonl em ~/.pi/agent/sessions/<slug>/ e so nasce no 1o turno.
    # Um path do layout do Claude aqui e um arquivo que NUNCA existe — e como o _jsonl_cache e de
    # CLASSE (api.registry + sse._registry), ele ficaria grudado nesse fantasma.
    registry.SessionRegistry._jsonl_cache.pop("s-pi", None)
    info, ns, _ = _create_pi(tmp_path, monkeypatch)
    assert info.provider == "pi"
    assert info.jsonl is None
    assert "s-pi" not in registry.SessionRegistry._jsonl_cache
    # spawn_command do PiAdapter, agora alcancavel: `pi --session-id <uuid>` (o `exec` vem do tmux.py)
    assert ns.call_args[0][2].startswith("pi --session-id ")


def test_create_pi_skips_the_claude_trust_list(tmp_path, monkeypatch):
    # _pretrust_cwd escreve hasTrustDialogAccepted no .claude.json, que o pi nem le.
    _, _, pt = _create_pi(tmp_path, monkeypatch)
    pt.assert_not_called()


def test_create_claude_still_seeds_the_same_path(tmp_path, monkeypatch):
    # Nao-regressao do caminho de TODO usuario de hoje: byte a byte o mesmo jsonl no cache.
    from unittest.mock import patch
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    registry.SessionRegistry._jsonl_cache.pop("cc", None)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as ns, \
         patch.object(registry, "_pretrust_cwd") as pt, \
         patch.object(registry.uuid, "uuid4", return_value=_UUID_PI):
        info = reg.create("cc", "/home/u/p")
    esperado = str(tmp_path / registry.sanitize_cwd("/home/u/p") / f"{_UUID_PI}.jsonl")
    assert info.jsonl == esperado
    assert registry.SessionRegistry._jsonl_cache["cc"] == esperado
    assert ns.call_args[0][2] == f"claude --session-id {_UUID_PI}"
    pt.assert_called_once_with("/home/u/p", None)


def test_create_pi_refuses_resume_instead_of_spawning_claude(tmp_path, monkeypatch):
    # O branch de resume monta `claude --resume <uuid>` LITERAL: aceitar aqui subiria um CLAUDE
    # lendo o transcript de outro agente, com cara de sessao Pi.
    from unittest.mock import patch
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as ns:
        with pytest.raises(ValueError) as exc:
            reg.create("s-pi", "/home/u/p", provider="pi", resume_session_id=_UUID_PI)
    assert "resume" in str(exc.value)
    ns.assert_not_called()


def test_create_pi_refuses_an_engine(tmp_path, monkeypatch):
    # `cp-engine --exec` so exporta ANTHROPIC_*/CLAUDE_CODE_*, que o pi ignora -> a sessao subiria
    # na conta do proprio pi PARECENDO estar no motor pedido.
    from unittest.mock import patch
    reg = registry.SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as ns, \
         patch("app.engines.listar", return_value={"kimi": {}}):
        with pytest.raises(ValueError) as exc:
            reg.create("s-pi", "/home/u/p", provider="pi", engine="kimi")
    assert "motor" in str(exc.value)
    ns.assert_not_called()


def test_session_file_accepts_a_fresh_ticket_before_the_file_exists(monkeypatch, tmp_path):
    # Sessao Pi recem-criada PELO APP: a extensao publica o bilhete no session_start, mas o Pi so
    # escreve o .jsonl no 1o turno. Exigir que o arquivo ja exista deixava a sessao "sem id" e
    # inclicavel no celular — e mandar a 1a mensagem era exatamente o que nao dava pra fazer.
    cfg = tmp_path / "cfg"
    (cfg / ".claude-pocket-pi").mkdir(parents=True)
    alvo = tmp_path / "2026-07-27T23-00-00-000Z_ainda-nao-existe.jsonl"   # NAO criado de proposito
    (cfg / ".claude-pocket-pi" / "300.json").write_text(
        json.dumps({"file": str(alvo), "id": "x", "ts": 2_000_000_000}))
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)
    monkeypatch.setattr(registry, "_proc_start_time", lambda pid: 1_999_999_000)  # bilhete e mais novo
    assert registry.pi_session_file("%300", pid=7, cwd="/w") == str(alvo)
