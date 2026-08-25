"""Hook de subagente (hooks/subagent_hook.py): o que ele grava a partir do payload real.

Os payloads abaixo são CÓPIA do que a CLI 2.1.234 entregou em 18/08/2026, capturado com um hook de
despejo — inclusive o detalhe que decide a feature: `agent_transcript_path` aponta pra um arquivo
PRÓPRIO do subagente (`<projeto>/<sessao>/subagents/agent-<id>.jsonl`), não pro transcript do pai.
"""
import importlib.util
import json
from pathlib import Path

import pytest

_MOD = Path(__file__).resolve().parent.parent / "hooks" / "subagent_hook.py"
_spec = importlib.util.spec_from_file_location("subagent_hook", _MOD)
hook = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(hook)


_TRANSCRIPT = "/home/u/.claude/projects/-home-u/55ccea64-756b-4883-813e-de7679e19973.jsonl"
_AGENTE_TRANSCRIPT = ("/home/u/.claude/projects/-home-u/55ccea64-756b-4883-813e-de7679e19973/"
                      "subagents/agent-aab93a1d36f986c2b.jsonl")


def _inicio(agent_id="aab93a1d36f986c2b", tipo="general-purpose") -> dict:
    return {
        "session_id": "a1a2e08c-e34b-4f8e-a693-2c1d084570e8",
        "transcript_path": _TRANSCRIPT,
        "cwd": "/home/u",
        "prompt_id": "742ff711-a2b4-4d8e-822d-967224075623",
        "agent_id": agent_id,
        "agent_type": tipo,
        "hook_event_name": "SubagentStart",
    }


def _fim(agent_id="aab93a1d36f986c2b", msg="PRONTO") -> dict:
    return {**_inicio(agent_id), "hook_event_name": "SubagentStop",
            "agent_transcript_path": _AGENTE_TRANSCRIPT, "last_assistant_message": msg,
            "effort": {"level": "high"}, "stop_hook_active": False, "background_tasks": []}


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    return tmp_path


class _StdinFalso:
    """stdin de mentira COM `.buffer`, como o de verdade.

    O hook lê `sys.stdin.buffer` e decodifica utf-8 na mão — um `io.StringIO` não tem `.buffer` e,
    pior, entregaria texto já decodificado, que é exatamente o passo onde o defeito morava (no
    Windows o modo texto usa cp1252; ver hooks/preview_hook.py). Fingir com bytes é o que faz este
    teste exercitar o caminho real.
    """

    def __init__(self, texto: str):
        import io
        self.buffer = io.BytesIO(texto.encode("utf-8"))

    def read(self) -> str:
        return self.buffer.read().decode("utf-8")


def _rodar(payload: dict, monkeypatch):
    """Executa o hook como ele roda de verdade: payload no stdin, em BYTES."""
    monkeypatch.setattr("sys.stdin", _StdinFalso(json.dumps(payload, ensure_ascii=False)))
    hook.main()


def _sidecar(casa: Path) -> dict:
    alvo = casa / ".hangar-subagents" / "55ccea64-756b-4883-813e-de7679e19973.json"
    return json.loads(alvo.read_text(encoding="utf-8"))


def test_start_registra_como_rodando(casa, monkeypatch):
    _rodar(_inicio(), monkeypatch)
    ag = _sidecar(casa)["agentes"]
    assert len(ag) == 1
    assert ag[0]["id"] == "aab93a1d36f986c2b"
    assert ag[0]["tipo"] == "general-purpose"
    # Sem `fim` = está rodando. É esse estado que faz o subagente aparecer no painel ENQUANTO
    # trabalha — o disco só o conhece depois que ele escreve.
    assert "fim" not in ag[0]


def test_stop_fecha_o_mesmo_agente_e_traz_o_transcript_dele(casa, monkeypatch):
    _rodar(_inicio(), monkeypatch)
    _rodar(_fim(msg="Achei os dados no Loki."), monkeypatch)
    ag = _sidecar(casa)["agentes"]
    assert len(ag) == 1, "Stop não pode criar uma segunda linha do mesmo agente"
    assert ag[0]["fim"] > ag[0]["inicio"]
    assert ag[0]["ultima_msg"] == "Achei os dados no Loki."
    # O caminho do transcript DO SUBAGENTE é o que permite mostrar o que ele fez, não só que existiu.
    assert ag[0]["transcript"] == _AGENTE_TRANSCRIPT
    assert "/subagents/agent-" in ag[0]["transcript"]


def test_stop_sozinho_tambem_registra(casa, monkeypatch):
    """Subagente que começou ANTES do hook ser instalado só dispara o Stop — e mesmo assim tem
    que aparecer, senão o painel esconde justo o histórico da sessão em curso."""
    _rodar(_fim(), monkeypatch)
    ag = _sidecar(casa)["agentes"]
    assert len(ag) == 1 and ag[0].get("fim")


def test_varios_agentes_convivem(casa, monkeypatch):
    for i in range(3):
        _rodar(_inicio(agent_id=f"ag{i}"), monkeypatch)
    _rodar(_fim(agent_id="ag1"), monkeypatch)
    ag = _sidecar(casa)["agentes"]
    assert [a["id"] for a in ag] == ["ag0", "ag1", "ag2"]
    assert [bool(a.get("fim")) for a in ag] == [False, True, False]


def test_poda_mantem_quem_esta_rodando(casa, monkeypatch):
    """O teto corta os ANTIGOS JÁ TERMINADOS. Podar quem está rodando apagaria exatamente o que o
    painel existe pra mostrar."""
    for i in range(hook._MAX + 10):
        _rodar(_inicio(agent_id=f"velho{i}"), monkeypatch)
        _rodar(_fim(agent_id=f"velho{i}"), monkeypatch)
    _rodar(_inicio(agent_id="vivo"), monkeypatch)
    ag = _sidecar(casa)["agentes"]
    assert len(ag) <= hook._MAX
    assert any(a["id"] == "vivo" for a in ag)


def test_mensagem_gigante_e_cortada(casa, monkeypatch):
    _rodar(_fim(msg="x" * 5000), monkeypatch)
    assert len(_sidecar(casa)["agentes"][0]["ultima_msg"]) == 400


def test_payload_sem_agent_id_nao_grava_nada(casa, monkeypatch):
    p = _inicio()
    del p["agent_id"]
    _rodar(p, monkeypatch)
    assert not (casa / ".hangar-subagents").exists()


def test_sidecar_corrompido_nao_derruba(casa, monkeypatch):
    alvo = casa / ".hangar-subagents" / "55ccea64-756b-4883-813e-de7679e19973.json"
    alvo.parent.mkdir(parents=True)
    alvo.write_text("{isto nao e json", encoding="utf-8")
    _rodar(_inicio(), monkeypatch)
    assert len(_sidecar(casa)["agentes"]) == 1


def test_chave_e_o_stem_do_transcript_nao_o_session_id(casa, monkeypatch):
    """É o mesmo casamento que os outros marcadores do app usam. Depois de um /clear o session_id
    continua o mesmo e o .jsonl muda — usar o id deixaria o painel preso na conversa antiga."""
    _rodar(_inicio(), monkeypatch)
    # `.lock` é a trava entre hooks concorrentes (ver `_trava`) — o que importa aqui é o SIDECAR.
    nomes = sorted(p.name for p in (casa / ".hangar-subagents").iterdir()
                   if p.suffix == ".json")
    assert nomes == ["55ccea64-756b-4883-813e-de7679e19973.json"]


def test_dois_hooks_ao_mesmo_tempo_nao_perdem_atualizacao(casa, monkeypatch):
    """Dois subagentes de um mesmo lote terminam quase juntos: são dois PROCESSOS de hook fazendo
    ler→mudar→gravar no mesmo arquivo. Sem a trava, o segundo grava por cima da alteração do
    primeiro e um agente some da lista — em silêncio, porque tudo aqui é fail-soft.

    Este caso já foi `skipif(os.name != "posix")` com a razão escrita: no Windows `_trava` devolvia
    None e a corrida abaixo REALMENTE perdia atualização, então o teste falharia por dizer a
    verdade. Era lacuna conhecida, e é ela que fechou (22/08/2026) — a trava lá é `msvcrt.locking`,
    o mesmo mecanismo de `contas.py`/`peers.py`. Conferido contra o código velho: com o `_trava`
    de antes este caso falha no Windows; com o de hoje passa nos dois. Um teste de trava que
    ninguém viu falhar não prova nada.

    O teste roda em THREADS (o hook é um processo, mas a trava é de arquivo e a corrida a proteger
    é a mesma) com uma pausa injetada entre a leitura e a escrita, que é a janela do defeito.
    Threads bastam nos dois sistemas porque `flock` é por descrição de arquivo aberto e
    `msvcrt.locking` é por handle: um segundo handle do MESMO processo é barrado igual.
    """
    import threading, time as _t
    original = hook._gravar

    def lento(alvo, agentes):
        _t.sleep(0.05)          # abre a janela: sem flock, o outro lê aqui e perde esta escrita
        original(alvo, agentes)

    monkeypatch.setattr(hook, "_gravar", lento)
    alvo = casa / ".hangar-subagents" / "55ccea64-756b-4883-813e-de7679e19973.json"

    def roda(agent_id):
        p = _inicio(agent_id=agent_id)
        fh = hook._trava(alvo)
        try:
            hook._atualizar(alvo, p, agent_id)
        finally:
            # Pelo helper do próprio hook, não por `fcntl` direto: quem destrava tem que usar o
            # mecanismo com que travou, e o teste não é o lugar de decidir qual dos dois é.
            hook._destravar(fh)

    ts = [threading.Thread(target=roda, args=(f"ag{i}",)) for i in range(4)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    ids = {a["id"] for a in _sidecar(casa)["agentes"]}
    assert ids == {"ag0", "ag1", "ag2", "ag3"}, f"perdeu atualizacao: {ids}"
