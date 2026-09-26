"""Uso de tools, skills e áreas lido do rollout do Codex — o par do `uso_claude`.

O Codex roda as tools DENTRO de um script (`custom_tool_call` "exec"): cada
`tools.exec_command({cmd: …})`, `tools.apply_patch("*** Update File: …")`, `tools.view_image`
é uma chamada. A saída volta UMA vez pro script inteiro; o tamanho é dividido igual entre as
tools dele (ponytail: sem casar cada pedaço da saída com a tool que o produziu).

Skill no Codex é sempre leitura de arquivo (`sed`/`cat` no SKILL.md): mesma regra de carga do
Claude. Resposta = `token_count` que mudou; compactação = registro `compacted`. Os tokens de cada
turno vêm do leitor de custos (`costs_sources.respostas_por_turno_codex`), que já resolve fork,
retomada e contador antigo — a área usa exatamente o que a tela de Custos soma.
"""
from __future__ import annotations

import json
import re
from dataclasses import replace
from datetime import datetime
from pathlib import Path

from app import uso_areas
from app.costs_claude_transcript import LOCAL
from app.uso_claude import Acumulador, UsoLinha, _int, comando_bash

_CHAMADA = re.compile(r"tools\.(\w+)\(")
_TEXTO_JS = r"""("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)"""
_CMD = re.compile(r"\bcmd\s*:\s*" + _TEXTO_JS)
_WORKDIR = re.compile(r"\bworkdir\s*:\s*" + _TEXTO_JS)
_PATH = re.compile(r"\bpath\s*:\s*" + _TEXTO_JS)
_PATCH = re.compile(r"\*\*\* (?:Update|Add|Delete) File: ([^\n\\\"]+)")
# Até onde procurar os argumentos de uma chamada depois do `tools.x(`.
_JANELA_ARGS = 4000


def _literal(s: str) -> str:
    if s[0] == '"':
        try:
            return json.loads(s)
        except ValueError:
            pass
    return s[1:-1].replace("\\'", "'").replace("\\`", "`").replace('\\"', '"').replace("\\\\", "\\")


def _chamadas(js: str) -> list[tuple[str, dict]]:
    out = []
    marcas = list(_CHAMADA.finditer(js))
    for i, m in enumerate(marcas):
        fim = marcas[i + 1].start() if i + 1 < len(marcas) else min(len(js), m.end() + _JANELA_ARGS)
        trecho = js[m.end():fim]
        info: dict = {}
        if m.group(1) == "exec_command":
            c, w = _CMD.search(trecho), _WORKDIR.search(trecho)
            info = {"cmd": _literal(c.group(1)) if c else "", "workdir": _literal(w.group(1)) if w else ""}
        elif m.group(1) == "apply_patch":
            info = {"arquivos": [p.strip() for p in _PATCH.findall(trecho)]}
        elif m.group(1) == "view_image":
            p = _PATH.search(trecho)
            info = {"path": _literal(p.group(1)) if p else ""}
        out.append((m.group(1), info))
    return out


def _texto_saida(saida) -> int:
    if isinstance(saida, str):
        return len(saida)
    if isinstance(saida, list):
        return sum(len(x.get("text") or "") for x in saida if isinstance(x, dict))
    return 0


class AcumuladorCodex(Acumulador):
    def __init__(self) -> None:
        super().__init__()
        self._sid = ""
        self._subagente = False
        self._inicio: datetime | None = None
        self._herdado = False
        self._turno = ""
        self._areas_turno: dict[str, dict[str, int]] = {}
        self._cwd_turno: dict[str, str] = {}
        self._scripts: dict[str, list[tuple[str, dict]]] = {}
        self._contador = None

    def registro(self, d: dict) -> None:
        p = d.get("payload")
        if not isinstance(p, dict):
            return
        tipo = d.get("type")
        quando = _quando(d.get("timestamp"))
        if quando:
            self._dia = quando.strftime("%Y-%m-%d")
        if tipo == "session_meta":
            ident = p.get("id") or p.get("session_id")
            if not self._sid:
                self._sid, self._cwd, self._inicio = ident or "", p.get("cwd") or "", quando
                fonte = p.get("source")
                self._subagente = (isinstance(fonte, dict) and "subagent" in fonte) or fonte == "subagent"
            elif ident and ident != self._sid:
                self._herdado = True               # o fork repete o histórico do pai
            return
        if tipo == "turn_context":
            if self._herdado and self._inicio and quando and quando >= self._inicio:
                self._herdado = False
            self._turno = p.get("turn_id") or self._turno
            self._cwd = p.get("cwd") or self._cwd
            self._model = p.get("model") if isinstance(p.get("model"), str) else self._model
            self._cwd_turno.setdefault(self._turno, self._cwd)
            return
        if tipo == "compacted":
            self._cargas, self._carregadas = [], {}
            return
        if self._herdado:
            return
        if tipo == "event_msg" and p.get("type") == "token_count":
            info = p.get("info")
            total = info.get("total_token_usage") if isinstance(info, dict) else None
            if isinstance(total, dict) and total != self._contador:
                self._contador = total
                # O peso da resposta sai do uso DELA: no Codex o cache lido vem dentro do input.
                ultima = info.get("last_token_usage")
                ultima = ultima if isinstance(ultima, dict) else {}
                lido = _int(ultima.get("cached_input_tokens"))
                self._resposta_nova({"input_tokens": max(0, _int(ultima.get("input_tokens")) - lido),
                                     "cache_read_input_tokens": lido})
            return
        if tipo != "response_item":
            return
        if p.get("type") == "custom_tool_call" and p.get("name") == "exec":
            chamadas = _chamadas(str(p.get("input") or ""))
            self._scripts[p.get("call_id") or ""] = chamadas
            for nome, info in chamadas:
                self._tool_codex(nome, info)
        elif p.get("type") == "custom_tool_call_output":
            self._saida(self._scripts.pop(p.get("call_id") or "", []), _texto_saida(p.get("output")))
        elif p.get("type") == "function_call" and isinstance(p.get("name"), str):
            self._somar("tool", p["name"], chamadas=1)
            if p["name"] == "spawn_agent":
                try:
                    args = json.loads(p.get("arguments") or "{}")
                except ValueError:
                    args = {}
                tipo_ag = args.get("agent_type") if isinstance(args, dict) else None
                self._somar("agente", str(tipo_ag or "spawn_agent"), chamadas=1, origem="sozinho")

    def _tool_codex(self, nome: str, info: dict) -> None:
        self._somar("tool", nome, chamadas=1)
        regras = uso_areas.regras_de(self._cwd)
        areas: set[str] = set()
        if nome == "exec_command" and info.get("cmd"):
            self._somar("bash", comando_bash(info["cmd"]), chamadas=1)
            areas = uso_areas.areas_do_comando(info["cmd"], info.get("workdir") or self._cwd, regras)
        elif nome == "apply_patch":
            areas = {uso_areas.area_do_caminho(a, self._cwd, regras) for a in info.get("arquivos", [])}
        elif nome == "view_image" and info.get("path"):
            self._somar("imagem", "lida:view_image", chamadas=1)
        pesos = self._areas_turno.setdefault(self._turno, {})
        for a in areas:
            pesos[a] = pesos.get(a, 0) + 1

    def _saida(self, chamadas: list[tuple[str, dict]], chars: int) -> None:
        if not chamadas:
            return
        parte = chars // len(chamadas)
        for nome, info in chamadas:
            cmd = info.get("cmd") or ""
            m = re.search(r"[^\s'\"`]*/skills/[^\s'\"`]+\.md", cmd) if "sed -i" not in cmd else None
            if m and self._ler_arquivo_de_skill(m.group(0), parte):
                continue
            self._somar("tool", nome, ctx_chars=parte)
            if cmd:
                self._somar("bash", comando_bash(cmd), ctx_chars=parte)

    def resultado_codex(self, por_turno: dict[str, list]) -> list[UsoLinha]:
        for turno, respostas in por_turno.items():
            contadas = self._areas_turno.get(turno) or {}
            pesos = contadas or {uso_areas.CONVERSA: 1}
            self._cwd = self._cwd_turno.get(turno, self._cwd)
            for i, r in enumerate(respostas):
                self._dia = r.ts.astimezone(LOCAL).strftime("%Y-%m-%d")
                self._model = r.model
                partes = {campo: uso_areas.repartir(getattr(r, campo), pesos)
                          for campo in ("input", "output", "cache_write", "cache_read")}
                for a in pesos:
                    self._somar("area", a, chamadas=contadas.get(a, 0) if i == 0 else 0, usage={
                        "input_tokens": partes["input"][a], "output_tokens": partes["output"][a],
                        "cache_creation_input_tokens": partes["cache_write"][a],
                        "cache_read_input_tokens": partes["cache_read"][a]})
        return sorted((replace(l, fonte="codex", session_id=self._sid, subagente=self._subagente) for l in self._linhas.values()),
                      key=lambda l: (l.dia, l.tipo, l.nome, l.detalhe))


def _quando(iso) -> datetime | None:
    if not isinstance(iso, str):
        return None
    try:
        return datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone(LOCAL)
    except ValueError:
        return None


def ler_rollout(arq: Path, por_turno: dict[str, list]) -> list[UsoLinha]:
    ac = AcumuladorCodex()
    try:
        f = arq.open(encoding="utf-8", errors="replace")
    except OSError:
        return []
    with f:
        for linha in f:
            try:
                d = json.loads(linha)
            except ValueError:
                continue
            if isinstance(d, dict):
                ac.registro(d)
    return ac.resultado_codex(por_turno)
