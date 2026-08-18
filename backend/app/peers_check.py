"""Checagem de um peer (Task 8) — "este endereço responde, e é esta máquina?".

Cola as primitivas das Tasks 3 e 5: `alcance.testar_endereco` responde "a porta responde?", e o
identificador gravado no peer (Task 5) responde "é a máquina que eu esperava?". O front mostra os
dois lados de um peer registrado a partir destes estados — nomeados, nunca um erro genérico.

Sem texto de interface aqui: o front traduz pelo `code` (régua da casa — nenhuma frase nasce no
Python). O teto de espera é o MESMO número do alcance (quem reusa herda o teto, não um segundo).

I/O num único seam (`_bater`), o mesmo desenho de `alcance._bater` — o teste troca ele e não toca
rede.
"""
from __future__ import annotations

import json
import time
import urllib.error
import urllib.request

from app import alcance, peers

# O MESMO teto da Task 3 — uma chamada de checagem é uma chamada de alcance.
TETO_ESPERA_S = alcance.TETO_ESPERA_S


def _bater(url: str, path: str = "/api/peers/identificador", token: str | None = None) -> tuple[int, dict | None]:
    """GET autenticado em `/api/peers/identificador` no peer. Devolve (status, corpo JSON|None).

    Usa a credencial DO PEER (a que o backend remoto espera): token do peers.json, resolvido em
    checar_peer pelo id do peer. Sem token guardado o GET sai sem Authorization — o remoto
    responde 401, que vira recusou (credencial não configurada é estado nomeado, não erro). O
    corpo pode ser None quando não-JSON. Levanta exceção de transporte (rede) — quem traduz é
    checar_peer.

    url vem do peers.json já com a barra final removida (peers.gravar_peer guarda sem barra).
    """
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(
        url + path,
        method="GET",
        headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=TETO_ESPERA_S) as r:
            status = r.status
            raw = r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        # O peer respondeu !2xx: é um estado nomeado (não é "não responde"), mas a identidade não
        # pode ser confirmada.
        try:
            corpo = json.loads(e.read().decode(errors="replace"))
        except (json.JSONDecodeError, ValueError):
            corpo = None
        return e.code, corpo
    try:
        return status, json.loads(raw)
    except (json.JSONDecodeError, ValueError):
        return status, None


def checar_peer(url: str, id_esperado: str) -> dict:
    """Estado de UM lado do peer. Devolve {estado, identificador, motivo, tempo_ms}.

    Estados nomeados (os do plano):
      ok        — o peer respondeu e o identificador dele BATE com o id_esperado
      estranho  — o peer respondeu, mas o identificador NÃO bate (outra máquina no endereço)
      falhou    — o peer não respondeu (timeout/recusa de conexão/erro de rede)
      recusou   — o peer respondeu 401: a credencial guardada foi recusada
      nao_configurado — URL vazia: nunca é testada (mesmo contrato de testar_endereco)

    URL vazia NUNCA é testada — volta "não configurado", que não é defeito.
    """
    if not url:
        return {"estado": "nao_configurado", "identificador": "", "motivo": "", "tempo_ms": None}
    inicio = time.monotonic()
    # Credencial DO PEER: o token que este servidor guardou para o id (a chave registrada no gesto
    # é o mesmo id usado aqui). Sem registro/token no peers.json, o GET sai sem auth e o remoto
    # responde 401 -> recusou: o estado certo pra "credencial não configurada".
    cfg = peers.peer_cfg(id_esperado)
    token = cfg[1] if cfg else None
    try:
        status, corpo = _bater(url, token=token)
    except Exception as e:
        return {
            "estado": "falhou",
            "identificador": "",
            "motivo": alcance._motivo(e),
            "tempo_ms": round((time.monotonic() - inicio) * 1000),
        }
    tempo = round((time.monotonic() - inicio) * 1000)
    if status == 401:
        return {"estado": "recusou", "identificador": "", "motivo": "credencial", "tempo_ms": tempo}
    # Respondeu HTTP mas a identidade não pode ser confirmada (corpo inválido, 404, outro shape):
    # é "estranho", não "falhou" — a porta está viva, só não é esta máquina (ou não diz quem é).
    if not isinstance(corpo, dict) or not isinstance(corpo.get("identificador"), str):
        return {"estado": "estranho", "identificador": "", "motivo": "identidade", "tempo_ms": tempo}
    if corpo["identificador"] != id_esperado:
        return {"estado": "estranho", "identificador": corpo["identificador"], "motivo": "identidade", "tempo_ms": tempo}
    return {"estado": "ok", "identificador": corpo["identificador"], "motivo": "", "tempo_ms": tempo}
