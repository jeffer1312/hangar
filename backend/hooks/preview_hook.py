#!/usr/bin/env python3
# ponytail: hook minimo — le o JSON do evento no stdin e grava o sidecar de PREVIA. SEM stdout.
# Falha em silencio (nunca trava o prompt). Espelha o padrao do state_hook.py. Usado pelo backend.
#
# E o publicador de previa do CLAUDE CODE — o par do scripts/pi/cp-state.ts do Pi, pelo MESMO
# contrato (<config>/.claude-pocket-preview/<stem>.json = {"text","ts"}), que preview.read_sidecar
# ja consome pra qualquer provider. O evento e o MessageDisplay (Claude Code >= 2.1.152): dispara
# ENQUANTO o texto do assistente e exibido, com `delta` INCREMENTAL — medido em 2.1.233, uma
# resposta de 5 paragrafos chegou em 6 eventos (index 0..5, final no ultimo), markdown cru.
#
# Regras herdadas do contrato (comentadas em app/preview.py):
#  - publica o ULTIMO bloco (a mensagem em voo), nao a soma do turno — a soma faria o
#    preview_is_committed ver o commitado como prefixo e engolir tudo;
#  - "" e RESPOSTA ("nao ha texto em voo", gravada no Stop), None/ausente e o que cai no pane;
#  - tmp com PID no nome antes do rename — o hook roda um processo por evento e dois eventos
#    proximos nao podem entrelacar bytes (a mesma armadilha ja medida na statusline).
# O acumulo entre eventos vive no PROPRIO sidecar (message_id gravado junto): cada processo le o
# arquivo, cola o delta se a mensagem e a mesma, ou recomeca se e outra. Perder um delta numa
# corrida read-modify-write so encurta a previa ate o proximo evento — best-effort, como o pane.
import json
import os
import sys
import time

_SUBDIR = ".claude-pocket-preview"


def _publicar(base: str, stem: str, payload: dict) -> None:
    d = os.path.join(base, _SUBDIR)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, f"{stem}.json.{os.getpid()}.tmp")
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, ensure_ascii=False)
    os.replace(tmp, os.path.join(d, stem + ".json"))


def _trava(base: str, stem: str):
    """Serializa o le-cola-grava entre processos do hook (um por evento). Sem isto, dois deltas
    consecutivos podiam ler o MESMO sidecar antes de qualquer um gravar — e o que perdesse a
    corrida sumia do MEIO da mensagem ate o Stop zerar (achado da review). Devolve o arquivo
    travado (a trava morre com o processo) ou None no Windows, que segue best-effort — la nao ha
    fcntl, e prever previa ocasionalmente curta e melhor que nao publicar."""
    try:
        import fcntl
    except ImportError:
        return None
    d = os.path.join(base, _SUBDIR)
    os.makedirs(d, exist_ok=True)
    fh = open(os.path.join(d, stem + ".lock"), "w")
    fcntl.flock(fh, fcntl.LOCK_EX)
    return fh


try:
    o = json.loads(sys.stdin.read())
    event = o.get("hook_event_name")
    # Chave = stem do transcript VIVO (o mesmo session_key que o backend usa), nao o session_id do
    # cmdline: numa sessao retomada os dois divergem e o backend le pelo jsonl real.
    tp = o.get("transcript_path") or ""
    stem = os.path.basename(tp)[:-6] if tp.endswith(".jsonl") else o.get("session_id")
    # basename SEMPRE, nos dois ramos: o stem vira nome de arquivo em _publicar, e um session_id
    # malformado com `/` ou `..` escaparia do .claude-pocket-preview/ (achado da review).
    stem = os.path.basename(stem) if isinstance(stem, str) else None
    base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
    if stem and event == "Stop":
        _lock = _trava(base, stem)
        _publicar(base, stem, {"text": "", "ts": time.time()})
    elif stem and event == "MessageDisplay" and not o.get("agent_id"):
        _lock = _trava(base, stem)  # cobre o le-cola-grava inteiro; solta na saida do processo
        # agent_id presente = texto de SUBAGENTE — nunca vira a previa da conversa principal.
        delta = o.get("delta")
        if isinstance(delta, str) and delta:
            mid = o.get("message_id")
            texto = delta
            if o.get("index"):  # index > 0 -> continuacao: cola no que ja foi publicado
                try:
                    with open(os.path.join(base, _SUBDIR, stem + ".json"),
                              encoding="utf-8") as fh:
                        ant = json.load(fh)
                except (OSError, ValueError):
                    ant = None  # sem anterior legivel -> publica so o delta (melhor curto que nada)
                # message_id DIFERENTE cai no "publica so o delta" — vale pro arquivo rotacionado
                # E pro sidecar de outra mensagem (mudou o mid no meio): nos dois casos o delta
                # sozinho e o melhor que este processo tem.
                if isinstance(ant, dict) and ant.get("message_id") == mid:
                    texto = str(ant.get("text", "")) + delta
                elif isinstance(ant, dict) and ant.get("text") == "":
                    # O "" e a marca do Stop: turno JA fechou. Um processo de MessageDisplay
                    # retardatario (fork+import lentos) chegando DEPOIS do Stop republicaria um
                    # rabo de mensagem como se estivesse em voo — melhor descartar o delta
                    # (achado da review). Mensagem nova de verdade comeca com index 0 e nao
                    # passa por aqui.
                    raise SystemExit(0)
            _publicar(base, stem, {"text": texto, "ts": time.time(), "message_id": mid})
except Exception:
    pass
sys.exit(0)
