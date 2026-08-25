#!/usr/bin/env python3
"""Hook de subagente: publica quem está rodando, e onde ler o que ele fez.

Por que existe (18/08/2026): o painel de atividade do app derivava os subagentes do TRANSCRIPT,
contando `tool_use` com nome `Agent`/`Workflow`. Isso deixou de cobrir o uso real: skill que forka
(`plugin:kubectl`) entra no transcript como `Skill`, e agente de FUNDO não entra como ferramenta
nenhuma — o app dizia "Waiting for 1 background agent to finish" sem conseguir dizer qual, nem o
que ele estava fazendo. Medido numa sessão do usuário: dois subagentes vivos, zero `Agent` no
transcript.

Os eventos `SubagentStart`/`SubagentStop` do Claude Code resolvem isso na fonte. O que eles
entregam (medido em 18/08/2026 contra a CLI 2.1.234, com um hook de despejo):

  SubagentStart  session_id, transcript_path, cwd, prompt_id, agent_id, agent_type
  SubagentStop   idem + agent_transcript_path, last_assistant_message, effort, background_tasks

`agent_transcript_path` é o achado que muda o que dá pra mostrar: o subagente tem arquivo PRÓPRIO,
em `<projeto>/<sessao>/subagents/agent-<id>.jsonl`. Com ele o app pode abrir a conversa do
subagente inteira, em vez de só contar quantos existem.

Contrato do sidecar — mesmo formato dos outros marcadores do app (`<config>/.hangar-*`),
chaveado pelo stem do .jsonl da SESSÃO:

    <config>/.hangar-subagents/<stem>.json
    {"agentes": [{"id","tipo","inicio","fim","ultima_msg","transcript"}], "ts"}

Três decisões que valem comentário:

1. O arquivo acumula e é PODADO por tamanho, não zerado a cada turno. Uma sessão longa dispara
   dezenas de subagentes, e zerar no fim de cada um faria o painel piscar entre "3 rodando" e
   "nenhum". `_MAX` corta os mais antigos JÁ TERMINADOS primeiro — quem está rodando nunca é podado,
   porque é justamente o que o painel precisa mostrar.
2. Escrita com tmp+rename levando o PID no nome do temporário. Dois subagentes que terminam no mesmo
   instante são dois processos de hook escrevendo o mesmo arquivo; com nome fixo, o `rename` promove
   bytes entrelaçados (o mesmo furo que `hangar_panel_common.py` e a statusline já corrigiram).
3. Falha aqui NUNCA pode atrapalhar a sessão: tudo é engolido e o hook sai 0. Um painel que não
   atualiza é um defeito de tela; um hook que estoura trava o turno de quem está trabalhando.
4. A sequência ler→mudar→gravar roda sob `flock` EXCLUSIVO. O tmp+rename do item 2 protege só a
   troca do arquivo; ele não impede que dois processos de hook leiam a MESMA lista e o segundo
   grave por cima da alteração do primeiro. E dois processos concorrentes é o caso normal, não a
   exceção: subagentes de um mesmo lote terminam quase juntos. Sem a trava, o sintoma é um agente
   preso em "rodando" pra sempre, ou sumido da lista — e em silêncio, porque tudo aqui é fail-soft.
"""
import json
import os
import sys
import time
from pathlib import Path

try:                    # POSIX: trava de arquivo de verdade
    import fcntl
except ImportError:     # Windows: não existe flock; a trava vai por msvcrt.locking (ver _trava)
    fcntl = None

try:                    # Windows: mesmo mecanismo que contas.py e peers.py já usam
    import msvcrt
except ImportError:
    msvcrt = None

_SUBDIR = ".hangar-subagents"
# Teto de itens guardados por sessão. 60 cobre com folga o pior caso visto (um lote de workflow) sem
# deixar o arquivo crescer sem fim numa sessão de horas.
_MAX = 60


def _config_dir() -> Path:
    env = os.environ.get("CLAUDE_CONFIG_DIR", "").strip()
    return Path(env) if env else Path.home() / ".claude"


def _stem(payload: dict) -> str | None:
    """Chave do sidecar: o stem do .jsonl da sessão — o mesmo que os outros marcadores usam."""
    caminho = payload.get("transcript_path")
    if not isinstance(caminho, str) or not caminho:
        # Sem transcript não há como casar com a sessão que o app mostra; o session_id é o plano B
        # (eles coincidem no caso normal, mas não depois de um /clear, que rebate o jsonl).
        sid = payload.get("session_id")
        return sid if isinstance(sid, str) and sid else None
    return Path(caminho).stem


def _ler(alvo: Path) -> list[dict]:
    try:
        o = json.loads(alvo.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    if not isinstance(o, dict):
        return []
    itens = o.get("agentes")
    return [x for x in itens if isinstance(x, dict)] if isinstance(itens, list) else []


def _podar(agentes: list[dict]) -> list[dict]:
    """Corta os mais antigos JÁ TERMINADOS. Quem está rodando fica, sempre: é o que o painel mostra."""
    if len(agentes) <= _MAX:
        return agentes
    rodando = [a for a in agentes if not a.get("fim")]
    prontos = [a for a in agentes if a.get("fim")]
    sobra = max(0, _MAX - len(rodando))
    return rodando + prontos[-sobra:] if sobra else rodando


def _gravar(alvo: Path, agentes: list[dict]) -> None:
    alvo.parent.mkdir(parents=True, exist_ok=True)
    tmp = alvo.with_suffix(f".tmp{os.getpid()}")
    tmp.write_text(json.dumps({"agentes": _podar(agentes), "ts": time.time()},
                              ensure_ascii=False), encoding="utf-8")
    os.replace(tmp, alvo)


def _trava(alvo: Path):
    """Trava exclusiva sobre a sequência ler→mudar→gravar. Arquivo de trava SEPARADO do alvo: o
    `os.replace` troca o inode do alvo, e uma trava tomada sobre o inode antigo não protegeria
    ninguém do processo seguinte.

    No Windows ela era NO-OP, e o item 4 do cabeçalho descreve exatamente o que isso custa: dois
    hooks concorrentes leem a MESMA lista e o segundo grava por cima do primeiro — agente preso em
    "rodando" pra sempre, ou sumido da lista, em silêncio. E concorrência aqui é o caso NORMAL:
    subagentes de um mesmo lote terminam quase juntos. O `msvcrt.locking` é o mesmo mecanismo que
    `contas.py` e `peers.py` usam; este hook não importa nenhum dos dois de propósito (ele roda
    standalone, pelo python do sistema, sem o pacote `app` no path), então a implementação é
    repetida aqui — é a única cópia justificada das três.

    Sem nenhum dos dois (plataforma exótica) devolve None e o hook segue sem trava: perder uma
    atualização de painel é melhor que não publicar nada."""
    if fcntl is None and msvcrt is None:
        return None
    alvo.parent.mkdir(parents=True, exist_ok=True)
    fh = open(alvo.with_suffix(".lock"), "a+")
    if fcntl is not None:
        fcntl.flock(fh.fileno(), fcntl.LOCK_EX)
    else:
        fh.seek(0)
        # LK_LOCK espera ~10s antes de desistir; LK_NBLCK falharia na hora e perderia a gravação
        # por causa de meio segundo de disputa.
        msvcrt.locking(fh.fileno(), msvcrt.LK_LOCK, 1)
    return fh


def _destravar(fh) -> None:
    """Solta a trava e fecha o arquivo. Aceita o None que `_trava` devolve na plataforma sem trava.

    Existe como função por um motivo prático: quem destrava tem que saber por qual dos dois
    mecanismos travou, e essa decisão não pode ficar espalhada — com ela repetida no `main` e no
    teste, o dia em que o ramo do Windows entrou o teste continuou chamando `fcntl` direto e
    estourava AttributeError lá, num caso que existe justamente pra exercitar a trava."""
    if fh is None:
        return
    if fcntl is not None:
        fcntl.flock(fh.fileno(), fcntl.LOCK_UN)
    else:
        fh.seek(0)
        msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)
    fh.close()


def main() -> None:
    try:
        # bytes + utf-8 explicito: em modo texto o Windows usa o locale (cp1252) e o prompt do
        # subagente chega corrompido na ActivitySheet. Ver preview_hook.py.
        payload = json.loads(sys.stdin.buffer.read().decode("utf-8", "replace") or "{}")
    except ValueError:
        return
    if not isinstance(payload, dict):
        return
    stem = _stem(payload)
    agente_id = payload.get("agent_id")
    if not stem or not isinstance(agente_id, str) or not agente_id:
        return

    alvo = _config_dir() / _SUBDIR / f"{stem}.json"
    fh = _trava(alvo)
    try:
        _atualizar(alvo, payload, agente_id)
    finally:
        _destravar(fh)


def _atualizar(alvo: Path, payload: dict, agente_id: str) -> None:
    agentes = _ler(alvo)
    evento = payload.get("hook_event_name")
    agora = time.time()

    achado = next((a for a in agentes if a.get("id") == agente_id), None)
    if achado is None:
        achado = {"id": agente_id, "inicio": agora}
        agentes.append(achado)

    achado["tipo"] = payload.get("agent_type") or achado.get("tipo") or "?"
    if evento == "SubagentStop":
        achado["fim"] = agora
        msg = payload.get("last_assistant_message")
        # Só a CAUDA da última mensagem: o painel mostra uma linha, e o texto inteiro de um subagente
        # de pesquisa são milhares de caracteres que iriam pro sidecar, pro SSE e pra memória do
        # navegador sem ninguém ler. Quem quiser tudo abre o transcript do agente.
        if isinstance(msg, str) and msg.strip():
            achado["ultima_msg"] = msg.strip()[:400]
        caminho = payload.get("agent_transcript_path")
        if isinstance(caminho, str) and caminho:
            achado["transcript"] = caminho

    try:
        _gravar(alvo, agentes)
    except OSError:
        pass


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # A promessa da docstring: o hook nunca atrapalha a sessão. Sem stderr também — o Claude Code
        # mostra saída de hook com erro na tela, e um painel quebrado não vale poluir o terminal.
        pass
    finally:
        sys.exit(0)
