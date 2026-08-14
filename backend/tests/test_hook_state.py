import json, time
from pathlib import Path
from app import hook_state


def _write(d: Path, sid: str, state: str):
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{sid}.json").write_text(json.dumps({"state": state, "ts": time.time()}))


def test_load_existing_seeds_map(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
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
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    _write(sd, "aaa", "idle")            # state flips
    hs._apply(sd / "aaa.json")
    assert hs.get_state("aaa")[0] == "idle"


def test_apply_ignores_bad_json(tmp_path):
    sd = tmp_path / ".claude-pocket-state"; sd.mkdir(parents=True)
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
    from app.state import corrige_ocioso_kimi

    wire = tmp_path / "wire.jsonl"
    # Fim de turno ABERTO: e o caso que a funcao existe pra pegar (prompt enfileirado na TUI nao
    # dispara hook, mas grava turn.prompt no wire).
    wire.write_text('{"type":"turn.prompt"}\n', encoding="utf-8")

    def com_mtime(mt):
        os.utime(wire, (mt, mt))
        return str(wire)

    corrige = corrige_ocioso_kimi

    # Turno andando: wire escrito DEPOIS do marcador ocioso.
    assert corrige(("idle", 1000.0), com_mtime(1090.0)) == ("working", 1090.0)

    # Ociosa de verdade: medido em 18 sessoes reais, o Stop chega no MESMO segundo da ultima linha.
    assert corrige(("idle", 1000.0), com_mtime(1000.0)) == ("idle", 1000.0)
    assert corrige(("idle", 1000.0), com_mtime(1001.5)) == ("idle", 1000.0)  # na folga
    assert corrige(("idle", 1000.0), com_mtime(999.0)) == ("idle", 1000.0)   # mais velho

    # So mexe em idle: awaiting_input pertence ao pane (a pergunta so existe la) e working ja esta
    # certo — promover qualquer um dos dois aqui seria inventar estado.
    alto = com_mtime(1090.0)
    assert corrige(("awaiting_input", 1000.0), alto) == ("awaiting_input", 1000.0)
    assert corrige(("working", 1000.0), alto) == ("working", 1000.0)

    # Sem marcador, sem caminho, ou caminho que nao existe: nunca inventa um estado.
    assert corrige(None, alto) is None
    assert corrige(("idle", 1000.0), None) == ("idle", 1000.0)
    assert corrige(("idle", 1000.0), str(tmp_path / "sumiu.jsonl")) == ("idle", 1000.0)

    # SEM teto de idade, de proposito: transcript velho continua valendo como "trabalhando". O teto
    # consertaria o Kimi que morre no meio do turno, mas ao preco de um turno legitimamente calado
    # (build longo) voltar a "idle" e disparar as automacoes em cima da sessao viva — ver a
    # docstring de corrige_ocioso_kimi. Erra-se pro lado visivel.
    assert corrige(("idle", 1000.0), com_mtime(1090.0)) == ("working", 1090.0)

    # Wire ilegivel/sem nenhum evento de turno: segue no mtime (comportamento antigo), nunca inventa
    # ociosidade por nao ter conseguido ler.
    mudo = tmp_path / "mudo.jsonl"
    mudo.write_text("{}\n", encoding="utf-8")
    os.utime(mudo, (1090.0, 1090.0))
    assert corrige(("idle", 1000.0), str(mudo)) == ("working", 1090.0)


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
    # Turno ANDANDO continua sendo working, com ou sem lixo depois do turn.prompt.
    assert corrige(ocioso, escreve('{"type":"turn.prompt"}', gordo)) == ("working", 1090.0)
    # turn.steer = usuario falando NO MEIO do turno -> segue aberto.
    assert corrige(ocioso, escreve('{"type":"turn.ended"}', '{"type":"turn.prompt"}',
                                   '{"type":"turn.steer"}')) == ("working", 1090.0)
    # turn.cancel (Esc) fecha, mesmo antes do turn.ended que sempre vem depois.
    assert corrige(ocioso, escreve('{"type":"turn.prompt"}', '{"type":"turn.cancel"}')) == ocioso
    # Mensagem CITANDO o nome do evento nao e fronteira: quem vale e o `type` de topo da linha.
    assert corrige(ocioso, escreve(
        '{"type":"turn.ended"}',
        '{"type":"context.append_message","text":"olha o \\"type\\":\\"turn.prompt\\" ali"}',
    )) == ocioso
