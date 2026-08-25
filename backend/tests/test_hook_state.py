import json, time
from pathlib import Path
from app import hook_state


def _write(d: Path, sid: str, state: str):
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{sid}.json").write_text(json.dumps({"state": state, "ts": time.time()}))


def test_load_existing_seeds_map(tmp_path):
    sd = tmp_path / ".hangar-state"
    _write(sd, "aaa", "working")
    _write(sd, "bbb", "idle")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"
    assert hs.get_state("bbb")[0] == "idle"


def test_get_state_none_when_absent(tmp_path):
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("missing") is None


def test_apply_updates_existing(tmp_path):
    sd = tmp_path / ".hangar-state"
    _write(sd, "aaa", "working")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    _write(sd, "aaa", "idle")            # state flips
    hs._apply(sd / "aaa.json")
    assert hs.get_state("aaa")[0] == "idle"


def test_apply_ignores_bad_json(tmp_path):
    sd = tmp_path / ".hangar-state"; sd.mkdir(parents=True)
    (sd / "x.json").write_text("{ not json")
    hs = hook_state.HookState()
    hs._apply(sd / "x.json")             # no raise
    assert hs.get_state("x") is None


# 13/08/2026: uma sessao Kimi apareceu "pronta" na lista e no chat por 18 minutos
# enquanto escrevia codigo. O marcador do hook estava congelado em idle desde as 08:38:35 — no Kimi,
# um turno que comeca a partir de um prompt ENFILEIRADO na TUI nao dispara UserPromptSubmit nem
# TurnStarted, e o pane tambem nao salva (o spinner e fase de lua, fora de SPINNER_GLYPHS). O
# transcript crescendo e a unica prova de vida.
def test_corrige_ocioso_kimi(tmp_path):
    import os
    from app.state import corrige_ocioso_kimi as corrige

    wire = tmp_path / "wire.jsonl"

    def com(*linhas, mtime=1090.0):
        wire.write_text("".join(ln + "\n" for ln in linhas), encoding="utf-8")
        os.utime(wire, (mtime, mtime))
        return str(wire)

    # Turno ABERTO no wire = trabalhando, e o MTIME NAO importa: e o caso do prompt enfileirado (nao
    # dispara hook) e o do main calado enquanto subagente trabalha.
    assert corrige(("idle", 1000.0), com('{"type":"turn.prompt"}', mtime=1090.0))[0] == "working"
    assert corrige(("idle", 1000.0), com('{"type":"turn.prompt"}', mtime=1000.0))[0] == "working"
    assert corrige(("idle", 1000.0), com('{"type":"turn.prompt"}', mtime=900.0))[0] == "working"

    # Turno FECHADO = parada, por mais novo que seja o arquivo (o `config.update` de 90KB que a
    # sessao grava parada foi o que derrubou o criterio por mtime — ver o teste abaixo).
    fechado = com('{"type":"turn.prompt"}', '{"type":"turn.ended"}', mtime=9999.0)
    assert corrige(("idle", 1000.0), fechado) == ("idle", 1000.0)

    # So mexe em idle: awaiting_input pertence ao pane (a pergunta so existe la) e working ja esta
    # certo — promover qualquer um dos dois aqui seria inventar estado.
    aberto = com('{"type":"turn.prompt"}')
    assert corrige(("awaiting_input", 1000.0), aberto) == ("awaiting_input", 1000.0)
    assert corrige(("working", 1000.0), aberto) == ("working", 1000.0)

    # Sem marcador, sem caminho, ou caminho que nao existe: nunca inventa um estado.
    assert corrige(None, aberto) is None
    assert corrige(("idle", 1000.0), None) == ("idle", 1000.0)
    assert corrige(("idle", 1000.0), str(tmp_path / "sumiu.jsonl")) == ("idle", 1000.0)

    # DEGRADADO: wire sem nenhuma fronteira de turno -> volta ao criterio antigo (mtime + folga),
    # que erra pro lado de "trabalhando". Nunca inventa ociosidade por nao ter conseguido ler.
    mudo = tmp_path / "mudo.jsonl"
    mudo.write_text("{}\n", encoding="utf-8")
    os.utime(mudo, (1090.0, 1090.0))
    assert corrige(("idle", 1000.0), str(mudo)) == ("working", 1090.0)
    os.utime(mudo, (1001.5, 1001.5))
    assert corrige(("idle", 1000.0), str(mudo)) == ("idle", 1000.0)   # dentro da folga

    # SEM teto de idade, de proposito: turno aberto e calado ha horas continua "trabalhando". O teto
    # consertaria o Kimi que morre no meio do turno, mas ao preco de um turno legitimamente calado
    # (build longo) voltar a "idle" e disparar as automacoes em cima da sessao viva. Erra-se pro
    # lado visivel: sessao morta o dono VE parada no pane; automacao escrevendo sozinha, nao.
    assert corrige(("idle", 1000.0), com('{"type":"turn.prompt"}', mtime=1.0))[0] == "working"


# 14/08/2026: sessao Kimi apareceu "em execucao" com o pane parado no prompt. O turno tinha fechado
# as 08:28:44 e as 08:40:46 o Kimi gravou um `config.update` (o system prompt inteiro, ~90KB) — o
# mtime sozinho leu isso como turno andando. Quem decide agora e a ultima FRONTEIRA de turno do wire.
def test_corrige_ocioso_kimi_escrita_que_nao_e_turno(tmp_path):
    import os
    from app.state import corrige_ocioso_kimi as corrige

    wire = tmp_path / "wire.jsonl"

    def escreve(*linhas, mtime=1090.0):
        wire.write_text("".join(ln + "\n" for ln in linhas), encoding="utf-8")
        os.utime(wire, (mtime, mtime))
        return str(wire)

    ocioso = ("idle", 1000.0)
    gordo = '{"type":"config.update","systemPrompt":"%s"}' % ("x" * 200_000)

    # O caso real: turno fechado, e DEPOIS uma escrita que nao e turno (inclusive maior que o chunk
    # de leitura, pra exercitar a montagem das linhas cortadas).
    assert corrige(ocioso, escreve('{"type":"turn.ended","turnId":4}', gordo)) == ocioso
    # Turno ANDANDO continua sendo working, com ou sem lixo depois do turn.prompt. O ts fica o do
    # marcador: com o turno aberto nao ha nada a medir (ninguem le esse ts — so o estado).
    assert corrige(ocioso, escreve('{"type":"turn.prompt"}', gordo)) == ("working", 1000.0)
    # turn.steer = usuario falando NO MEIO do turno -> segue aberto.
    assert corrige(ocioso, escreve('{"type":"turn.ended"}', '{"type":"turn.prompt"}',
                                   '{"type":"turn.steer"}')) == ("working", 1000.0)
    # turn.cancel (Esc) fecha, mesmo antes do turn.ended que sempre vem depois.
    assert corrige(ocioso, escreve('{"type":"turn.prompt"}', '{"type":"turn.cancel"}')) == ocioso
    # Mensagem CITANDO o nome do evento nao e fronteira: quem vale e o `type` de topo da linha.
    assert corrige(ocioso, escreve(
        '{"type":"turn.ended"}',
        '{"type":"context.append_message","text":"olha o \\"type\\":\\"turn.prompt\\" ali"}',
    )) == ocioso


# 14/08/2026, 3a vez que a MESMA sessao aparece "pronta" trabalhando — desta vez com o terminal
# mostrando "Running 2 agents (1 done, 1 running)". Duas coisas ao mesmo tempo:
#  - o main DELEGOU pra subagentes (tool `Agent`), que escrevem no wire DELES; o main fica calado o
#    turno inteiro, entao o mtime do main nao prova nada;
#  - um subagente terminou e o Stop disparou com o session_id da SESSAO (subagente roda no mesmo
#    processo), marcando `idle` no meio do turno do main.
# Com o marcador e o mtime do main no mesmo segundo, o portao antigo nem olhava a fronteira.
def _sessao_kimi(tmp_path, main_linhas, main_mtime, subs=()):
    import os
    agents = tmp_path / "sessao" / "agents"
    (agents / "main").mkdir(parents=True)
    main = agents / "main" / "wire.jsonl"
    main.write_text("".join(ln + "\n" for ln in main_linhas), encoding="utf-8")
    os.utime(main, (main_mtime, main_mtime))
    for nome, mt in subs:
        (agents / nome).mkdir()
        w = agents / nome / "wire.jsonl"
        w.write_text('{"type":"metadata"}\n', encoding="utf-8")
        os.utime(w, (mt, mt))
    return str(main)


def test_kimi_trabalhando_em_subagente_nao_e_ociosa(tmp_path):
    from app.state import corrige_ocioso_kimi

    # main parado no meio do turno (fronteira ABERTA), subagente escrevendo agora.
    main = _sessao_kimi(tmp_path, ['{"type":"turn.prompt"}'], 1000.0,
                        subs=[("agent-0", 1000.0), ("agent-1", 1240.0)])
    # marcador `idle` gravado DEPOIS da ultima escrita do main (o Stop do subagente que terminou) —
    # e exatamente a combinacao que o criterio antigo lia como "parada". O ts nao muda: quem le este
    # marcador so olha o estado (ver o comentario no fim de corrige_ocioso_kimi).
    assert corrige_ocioso_kimi(("idle", 1000.5), main) == ("working", 1000.5)


def test_kimi_turno_fechado_continua_ociosa_com_subagente_novo(tmp_path):
    from app.state import corrige_ocioso_kimi

    # Turno FECHADO: nem mtime de subagente ressuscita (sobra de agente em background nao e turno).
    main = _sessao_kimi(tmp_path, ['{"type":"turn.prompt"}', '{"type":"turn.ended"}'], 1000.0,
                        subs=[("agent-0", 9000.0)])
    assert corrige_ocioso_kimi(("idle", 1000.0), main) == ("idle", 1000.0)


def test_kimi_mtime_da_sessao_pega_o_mais_novo(tmp_path):
    from app.state import _kimi_mtime_da_sessao

    main = _sessao_kimi(tmp_path, ['{"type":"turn.prompt"}'], 1000.0,
                        subs=[("agent-0", 1500.0), ("agent-1", 1200.0)])
    assert _kimi_mtime_da_sessao(main) == 1500.0
    # Fora do layout do Kimi (pasta avo != "agents"), usa o mtime do proprio arquivo — sem varrer
    # pasta alheia atras de wire.jsonl que nao e desta sessao.
    solto = tmp_path / "wire.jsonl"
    solto.write_text("{}\n", encoding="utf-8")
    import os
    os.utime(solto, (777.0, 777.0))
    assert _kimi_mtime_da_sessao(str(solto)) == 777.0
