"""Uso de tools, skills, agentes e contexto injetado, lido do transcript do Claude Code.

Roda na MESMA passada que a leitura de tokens (`costs_claude_transcript.ler_transcript`): o
transcript é o arquivo mais pesado da máquina, e lê-lo duas vezes dobraria a varredura fria.
Este módulo só acumula a partir das linhas já decodificadas; quem itera o arquivo é o outro.

O que é medido de verdade e o que é estimado:
- chamadas (tool, skill, agente) e ocorrências (contexto): contagem exata.
- skill: cada CARGA é um texto que entra no contexto e fica lá até o fim do arquivo ou a
  compactação — pela ferramenta Skill, por barra, pelo hook que cola a skill na abertura, por
  Read/Bash no `SKILL.md`, ou pela reinjeção depois de compactar. Arquivos da pasta da skill lidos
  DEPOIS da carga (references/) somam a ela. `ocupados` = chars × respostas em que o texto esteve
  no contexto (exato em chars e em respostas; vira tokens no agregador). `ocupados_eq` pesa cada
  resposta pela mistura real de entrada, cache escrito e cache lido dela.
- `ctx_chars` (resultado de tool, texto injetado por hook/skill/instruções): caracteres do que
  entrou no contexto; vira "tokens estimados" na tela, nunca dólar. O transcript não carrega
  contagem de tokens por bloco, e tokenizador de outro provedor erraria mais do que chars/4.
- attachment: `rendered` é o que a CLI mostrou ao modelo (≥ 2.1.26x); sem ele, `content`/`text`.
  `stdout` de hook sem `content` NÃO entra no contexto e não é medido.
"""
from __future__ import annotations

import base64
import re
from collections import defaultdict
import struct
from dataclasses import asdict, dataclass, replace

from app import uso_areas

# Um tool_use `Bash` vira `bash:<comando>`; estes prefixos não são o comando.
_PREFIXOS_BASH = {"sudo", "env", "time", "timeout", "rtk", "command", "exec", "nohup", "nice"}
_ROTULO_HOOK_CHARS = 60
# Proporção de preço da Anthropic em relação ao token de entrada novo.
_PESO_CACHE_WRITE = 1.25
_PESO_CACHE_READ = 0.1
_CAMPOS_USAGE = ("input_tokens", "output_tokens", "cache_creation_input_tokens",
                 "cache_read_input_tokens", "cache_1h")


@dataclass(frozen=True)
class UsoLinha:
    dia: str            # YYYY-MM-DD no fuso local
    cwd: str
    model: str
    tipo: str           # tool | bash | mcp | skill | agente | contexto | imagem | area
    nome: str
    plugin: str = ""    # prefixo de skill/hook (ecc, superpowers…) quando há
    detalhe: str = ""   # mcp: tool completo; agente: agentId (liga ao transcript filho)
    # Quem pediu. skill: "voce" (/skill digitado), "modelo" (ferramenta Skill), "leitura" (Read/
    # Bash no SKILL.md), "hook" (colada na abertura) ou "compactacao" (reinjetada) — exato.
    # agente: "pedido" ou "sozinho" — HEURÍSTICA pelo prompt do turno (ver `_pede_agente`).
    origem: str = ""
    chamadas: int = 0
    ctx_chars: int = 0
    # Tokens já estimados por outra régua que não chars/4: imagem = largura×altura÷750 (regra
    # da Anthropic), lida do cabeçalho do PNG. Soma ao chars/4 no agregador.
    tokens_est: int = 0
    input: int = 0
    output: int = 0
    cache_write: int = 0
    cache_read: int = 0
    cache_write_1h: int = 0
    fast: bool = False
    # skill: chars × respostas com o texto no contexto, e quantas respostas foram.
    ocupados: int = 0
    respostas: int = 0
    # skill: o mesmo, pesado pelo que cada resposta pagou (cache lido vale 0,1 do token novo).
    ocupados_eq: int = 0
    fonte: str = "claude"   # claude | codex — decide a tarifa no custo
    subagente: bool = False  # transcript/rollout de subagente: não conta como sessão
    session_id: str = ""
    conta: str = ""     # identidade da conta (anthropic:<uuid>), aplicada depois do cache

    def para_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def de_dict(cls, d: dict) -> "UsoLinha | None":
        try:
            return cls(**d)
        except TypeError:
            return None


def _int(v) -> int:
    try:
        return int(v or 0)
    except (TypeError, ValueError):
        return 0


def _texto(c) -> int:
    """Tamanho em chars de um conteúdo que pode ser string, lista de blocos ou nada."""
    if isinstance(c, str):
        return len(c)
    if isinstance(c, list):
        n = 0
        for b in c:
            if isinstance(b, dict):
                n += _texto(b["text"] if isinstance(b.get("text"), str) else b.get("content"))
            elif isinstance(b, str):
                n += len(b)
        return n
    return 0


_PALAVRAS_SHELL = {"do", "then", "else", "done", "fi", "in", "cd"}


def comando_bash(cmd: str) -> str:
    """Primeira palavra útil de um comando: pula `cd x &&`, variáveis (`X=$(cmd …` vale o
    `cmd`), `(`, flags soltas e sudo/env/rtk."""
    for trecho in _segmentos(cmd):
        for tok in trecho.split():
            t = tok.lstrip("(")
            if "=" in t and not t.startswith("="):
                valor = t.split("=", 1)[1]
                if not valor.startswith("$("):
                    continue
                t = valor[2:]
            # `timeout 300 npx …`: o prefixo e o número dele não são o comando.
            if not t or t.startswith("-") or t[0].isdigit() or t in _PREFIXOS_BASH:
                continue
            if t in _PALAVRAS_SHELL:
                break
            return t.rsplit("/", 1)[-1]
    return "?"


def _segmentos(cmd: str) -> list[str]:
    out, atual = [], []
    i = 0
    while i < len(cmd):
        dois = cmd[i:i + 2]
        if dois in ("&&", "||") or cmd[i] in ";|\n":
            out.append("".join(atual))
            atual = []
            i += 2 if dois in ("&&", "||") else 1
            continue
        atual.append(cmd[i])
        i += 1
    out.append("".join(atual))
    return [s for s in out if s.strip()]


# Palavras no prompt do usuário que indicam que ELE pediu subagente. É heurística: quem pede
# de forma indireta ("pesquisa isso em paralelo") pode cair em "sozinho".
_PEDE_AGENTE = re.compile(
    r"\b(sub-?agentes?|agentes?|agents?|explore|paralelo|parallel|dispara|delega|"
    r"workflow|fan-?out|subagent-driven)\b", re.IGNORECASE)


def _pede_agente(prompt: str) -> bool:
    return bool(_PEDE_AGENTE.search(prompt))


def tokens_de_imagem(bloco: dict) -> tuple[int, int]:
    """(chars de base64, tokens estimados) de um bloco `image`. PNG tem largura e altura nos
    bytes 16–24 do cabeçalho; outros formatos ficam sem estimativa (0), só contam."""
    src = bloco.get("source") if isinstance(bloco.get("source"), dict) else {}
    dados = src.get("data") if isinstance(src.get("data"), str) else ""
    tokens = 0
    if src.get("media_type") == "image/png" and len(dados) >= 32:
        try:
            cabecalho = base64.b64decode(dados[:32])
            if cabecalho[:8] == b"\x89PNG\r\n\x1a\n":
                largura, altura = struct.unpack(">II", cabecalho[16:24])
                tokens = largura * altura // 750
        except (ValueError, struct.error):
            tokens = 0
    return len(dados), tokens


def plugin_de(nome: str) -> str:
    return nome.split(":", 1)[0] if ":" in nome else ""


def plugin_de_hook(primeira_linha: str) -> str:
    """Hook não diz de que plugin é; a primeira linha do que ele injeta costuma dizer:
    `[skill-suggester] …`, `PONYTAIL MODE ACTIVE`. Menção ao nome no MEIO do texto não conta
    (o aviso do last30days cita "superpowers" e não é dela)."""
    p = primeira_linha.strip()
    if p.startswith("[") and "]" in p:
        return p[1:p.index("]")].strip()
    if " MODE ACTIVE" in p:
        return p.split(" ", 1)[0].lower()
    return ""


# Hook que cola uma skill inteira (SessionStart do superpowers).
_SKILL_NO_HOOK = re.compile(r"full content of your '([\w.:-]+)' skill")
_ARQ_SKILL = re.compile(r"[^\s'\"`]*/skills/[^\s'\"`]+\.md")
# Menos que isso não é o texto da skill (escrita por heredoc, erro de arquivo inexistente).
_MIN_CHARS_LEITURA = 50
_PASTAS_DE_APOIO ={"references", "reference", "scripts", "assets", "templates", "examples"}


def skill_do_caminho(caminho: str) -> tuple[str, bool] | None:
    """(nome da skill, é o SKILL.md) de um arquivo dentro da pasta de uma skill.

    Nome = pasta do SKILL.md (ou a que contém `references/`…); plugin pelo cache
    (`plugins/cache/<marketplace>/<plugin>/`) ou pelo marketplace (`<nome>-marketplace`)."""
    partes = caminho.replace("\\", "/").split("/")
    if "skills" not in partes or not partes[-1].endswith(".md"):
        return None
    e_skill_md = partes[-1] == "SKILL.md"
    if e_skill_md:
        nome = partes[-2]
    else:
        apoio = [i for i, p in enumerate(partes) if p in _PASTAS_DE_APOIO]
        ultimo_skills = len(partes) - 1 - partes[::-1].index("skills")
        i = apoio[-1] if apoio else ultimo_skills + 2
        if i - 1 <= ultimo_skills or i > len(partes) - 1:
            return None
        nome = partes[i - 1]
    plugin = ""
    if "plugins" in partes:
        j = partes.index("plugins")
        if partes[j + 1:j + 2] == ["cache"] and len(partes) > j + 3:
            plugin = partes[j + 3]
        elif partes[j + 1:j + 2] == ["marketplaces"] and len(partes) > j + 2:
            # ponytail: marketplace de um plugin só se chama `<plugin>-marketplace`.
            plugin = partes[j + 2].removesuffix("-marketplace")
    return (f"{plugin}:{nome}" if plugin else nome), e_skill_md


class Acumulador:
    """Recebe cada linha decodificada do transcript, na ordem do arquivo."""

    def __init__(self) -> None:
        self._linhas: dict[tuple, UsoLinha] = {}
        self._tools: dict[str, tuple[str, dict]] = {}      # tool_use id -> (nome, input)
        self._respostas_vistas: set = set()
        # Skill chamada (ferramenta ou barra) esperando o texto dela entrar: (nome, origem, chave).
        self._skill_pendente: tuple | None = None
        # Cargas no contexto agora: [chave da linha, chars]; zera na compactação.
        self._cargas: list[list] = []
        self._carregadas: dict[str, tuple] = {}             # nome -> chave (arquivos de apoio)
        self._prompt_id = None
        self._pediu_agente = False                          # o prompt em curso fala em agente?
        self._dia = ""
        self._cwd = ""
        self._model = ""
        # Áreas: tools por área de cada turno (um promptId) e, por resposta, (turno, grupo, usage).
        self._turnos: list[dict[str, int]] = [{}]
        self._respostas: dict[tuple, tuple[int, tuple, dict]] = {}

    def _carregar(self, nome: str, origem: str, chars: int, chamadas: int = 1,
                  chave: tuple | None = None) -> None:
        if chave is None:
            chave = self._somar("skill", nome, plugin=plugin_de(nome), origem=origem,
                                chamadas=chamadas, ctx_chars=chars)
        else:
            self._linhas[chave] = replace(self._linhas[chave],
                                          ctx_chars=self._linhas[chave].ctx_chars + chars)
        if chars > 0:
            self._cargas.append([chave, chars])
        self._carregadas[nome] = chave

    def _resposta_nova(self, u: dict) -> None:
        # A skill paga a mistura da resposta: quase sempre releitura de cache, e regravação
        # inteira quando o cache expirou na espera.
        i = _int(u.get("input_tokens"))
        cw = _int(u.get("cache_creation_input_tokens"))
        cr = _int(u.get("cache_read_input_tokens"))
        total = i + cw + cr
        peso = (i + _PESO_CACHE_WRITE * cw + _PESO_CACHE_READ * cr) / total if total else 1.0
        vistas = set()
        for chave, chars in self._cargas:
            l = self._linhas[chave]
            self._linhas[chave] = replace(l, ocupados=l.ocupados + chars,
                                          ocupados_eq=l.ocupados_eq + round(chars * peso),
                                          respostas=l.respostas + (chave not in vistas))
            vistas.add(chave)

    def _somar(self, tipo: str, nome: str, *, plugin: str = "", detalhe: str = "",
               origem: str = "", chamadas: int = 0, ctx_chars: int = 0, tokens_est: int = 0,
               usage: dict | None = None) -> tuple:
        chave = (self._dia, self._cwd, self._model, tipo, nome, plugin, detalhe, origem)
        antes = self._linhas.get(chave) or UsoLinha(
            dia=self._dia, cwd=self._cwd, model=self._model, tipo=tipo, nome=nome,
            plugin=plugin, detalhe=detalhe, origem=origem)
        u = usage or {}
        criacao = u.get("cache_creation")
        cache_1h = _int(criacao.get("ephemeral_1h_input_tokens")) if isinstance(criacao, dict) else 0
        self._linhas[chave] = replace(
            antes, chamadas=antes.chamadas + chamadas, ctx_chars=antes.ctx_chars + ctx_chars,
            tokens_est=antes.tokens_est + tokens_est,
            input=antes.input + _int(u.get("input_tokens")),
            output=antes.output + _int(u.get("output_tokens")),
            cache_write=antes.cache_write + _int(u.get("cache_creation_input_tokens")),
            cache_read=antes.cache_read + _int(u.get("cache_read_input_tokens")),
            cache_write_1h=antes.cache_write_1h + min(max(0, cache_1h),
                                                      max(0, _int(u.get("cache_creation_input_tokens")))),
            fast=antes.fast or u.get("speed") == "fast")
        return chave

    def linha(self, d: dict, dia: str) -> None:
        tipo = d.get("type")
        if dia:
            self._dia = dia
        if isinstance(d.get("cwd"), str):
            self._cwd = d["cwd"]
        if d.get("subtype") == "compact_boundary":
            # O resumo substitui o contexto: o que estava carregado sai (e volta por invoked_skills).
            self._cargas = []
            self._carregadas = {}
        elif tipo == "assistant":
            self._assistant(d)
        elif tipo == "user":
            self._user(d)
        elif tipo == "attachment":
            self._attachment(d)

    def _assistant(self, d: dict) -> None:
        msg = d.get("message")
        if not isinstance(msg, dict):
            return
        if isinstance(msg.get("model"), str):
            self._model = msg["model"]
        usage = msg.get("usage") if isinstance(msg.get("usage"), dict) else None
        # Blocos da mesma resposta repetem o usage: a resposta conta uma vez nas cargas.
        ident = (d.get("requestId"), msg.get("id")) if msg.get("id") else None
        if usage:
            self._turno_somar(usage, ident)
            if ident is None or ident not in self._respostas_vistas:
                self._resposta_nova(usage)
        if ident:
            self._respostas_vistas.add(ident)
        for b in msg.get("content") or []:
            if not isinstance(b, dict) or b.get("type") != "tool_use":
                continue
            nome = b.get("name")
            if not isinstance(nome, str):
                continue
            entrada = b.get("input") if isinstance(b.get("input"), dict) else {}
            if isinstance(b.get("id"), str):
                self._tools[b["id"]] = (nome, entrada)
            self._turno_tool(nome, entrada)
            if nome == "Skill" and isinstance(entrada.get("skill"), str):
                skill = entrada["skill"]
                self._skill_pendente = (skill, "modelo", self._somar(
                    "skill", skill, plugin=plugin_de(skill), origem="modelo", chamadas=1))
            elif nome == "Agent":
                tipo_ag = entrada.get("subagent_type") or "general-purpose"
                self._somar("agente", str(tipo_ag), chamadas=1,
                            origem="pedido" if self._pediu_agente else "sozinho")
            elif nome == "Bash" and isinstance(entrada.get("command"), str):
                self._somar("bash", comando_bash(entrada["command"]), chamadas=1)
            elif nome.startswith("mcp__"):
                partes = nome.split("__", 2)
                servidor = partes[1] if len(partes) > 1 else nome
                self._somar("mcp", servidor, detalhe=nome, chamadas=1)
            self._somar("tool", nome, chamadas=1)

    def _user(self, d: dict) -> None:
        msg = d.get("message")
        conteudo = msg.get("content") if isinstance(msg, dict) else None
        prompt_id = d.get("promptId")
        if prompt_id and prompt_id != self._prompt_id:
            self._turnos.append({})
            self._prompt_id = prompt_id
            self._skill_pendente = None
        if isinstance(conteudo, str):
            self._pediu_agente = _pede_agente(conteudo)
            self._comando(conteudo)
            return
        if not isinstance(conteudo, list):
            return
        if not d.get("isMeta"):
            texto = " ".join(b["text"] for b in conteudo
                             if isinstance(b, dict) and b.get("type") == "text"
                             and isinstance(b.get("text"), str))
            if texto:
                self._pediu_agente = _pede_agente(texto)
        for b in conteudo:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "image":
                # ctx_chars fica 0: base64 não é texto, e chars/4 inflaria o que os pixels já medem.
                _, tokens = tokens_de_imagem(b)
                self._somar("imagem", "enviada", chamadas=1, tokens_est=tokens)
            if b.get("type") == "tool_result":
                nome, entrada = self._tools.get(b.get("tool_use_id"), ("?", {}))
                chars = _texto(b.get("content"))
                # Texto de skill lido como arquivo é da skill: não soma de novo no Read/Bash.
                do_tool = 0 if self._leitura_de_skill(nome, entrada, chars) else chars
                self._somar("tool", nome, ctx_chars=do_tool)
                if isinstance(b.get("content"), list):
                    for x in b["content"]:
                        if isinstance(x, dict) and x.get("type") == "image":
                            _, it = tokens_de_imagem(x)
                            self._somar("imagem", f"lida:{nome}", chamadas=1, tokens_est=it)
                if nome == "Bash" and isinstance(entrada.get("command"), str):
                    self._somar("bash", comando_bash(entrada["command"]), ctx_chars=do_tool)
                elif nome.startswith("mcp__"):
                    partes = nome.split("__", 2)
                    self._somar("mcp", partes[1] if len(partes) > 1 else nome, detalhe=nome,
                                ctx_chars=chars)
                elif (nome == "Skill" and self._skill_pendente and self._skill_pendente[2]
                      and chars >= _MIN_CHARS_LEITURA):
                    # Formato antigo: o texto da skill vinha no próprio resultado.
                    s, origem, chave = self._skill_pendente
                    self._carregar(s, origem, chars, chave=chave)
                elif nome == "Agent":
                    r = d.get("toolUseResult")
                    agent_id = r.get("agentId") if isinstance(r, dict) else None
                    if isinstance(agent_id, str):
                        tipo_ag = entrada.get("subagent_type") or "general-purpose"
                        self._somar("agente", str(tipo_ag), detalhe=agent_id)
            elif b.get("type") == "text" and isinstance(b.get("text"), str):
                if "<command-name>" in b["text"]:
                    self._comando(b["text"])
                elif d.get("isMeta") and self._skill_pendente:
                    # Texto expandido da skill: é o que entrou no contexto. Por barra, a linha só
                    # nasce aqui — comando embutido (/model, /clear) não expande e não é skill.
                    s, origem, chave = self._skill_pendente
                    if chave is None:
                        chave = self._somar("skill", s, plugin=plugin_de(s), origem=origem, chamadas=1)
                        self._skill_pendente = (s, origem, chave)
                    self._carregar(s, origem, len(b["text"]), chave=chave)

    def _leitura_de_skill(self, nome: str, entrada: dict, chars: int) -> bool:
        """Read/Bash num arquivo de skill: `SKILL.md` é uma carga; arquivo de apoio só conta se a
        skill já está carregada (senão é alguém editando a skill, não usando)."""
        if nome == "Read":
            caminho = entrada.get("file_path")
        elif nome == "Bash" and isinstance(entrada.get("command"), str) and "sed -i" not in entrada["command"]:
            m = _ARQ_SKILL.search(entrada["command"])
            caminho = m.group(0) if m else None
        else:
            return False
        return self._ler_arquivo_de_skill(caminho, chars)

    def _ler_arquivo_de_skill(self, caminho, chars: int) -> bool:
        achado = skill_do_caminho(caminho) if isinstance(caminho, str) else None
        if not achado or chars < _MIN_CHARS_LEITURA:
            return False
        s, e_skill_md = achado
        if e_skill_md:
            self._carregar(s, "leitura", chars)
            return True
        if s in self._carregadas:
            self._carregar(s, "", chars, chave=self._carregadas[s])
            return True
        return False

    def _comando(self, texto: str) -> None:
        ini = texto.find("<command-name>")
        if ini < 0:
            return
        fim = texto.find("</command-name>", ini)
        nome = texto[ini + len("<command-name>"):fim].strip() if fim > ini else ""
        if nome:
            self._skill_pendente = (nome.lstrip("/"), "voce", None)

    def _attachment(self, d: dict) -> None:
        a = d.get("attachment")
        if not isinstance(a, dict) or not isinstance(a.get("type"), str):
            return
        rendered = d.get("rendered")
        if rendered is not None:
            chars = _texto(rendered)
        else:
            chars = _texto(a.get("content")) or _texto(a.get("text"))
        tipo = a["type"]
        plugin = ""
        if tipo == "invoked_skills" and isinstance(a.get("skills"), list):
            for s in a["skills"]:
                if isinstance(s, dict) and isinstance(s.get("name"), str):
                    self._carregar(s["name"], "compactacao", _texto(s.get("content")), chamadas=0)
            return
        if tipo.startswith("hook") and chars == 0:
            return      # hook que não injetou nada não entrou no contexto: não é ocorrência
        if tipo.startswith("hook"):
            bruto = a.get("content")
            inicio = "".join(x if isinstance(x, str) else x.get("text", "") if isinstance(x, dict) else ""
                             for x in (bruto if isinstance(bruto, list) else [bruto]))[:600]
            m = _SKILL_NO_HOOK.search(inicio) if isinstance(inicio, str) else None
            if m:
                self._carregar(m.group(1), "hook", chars)
                return
            evento = a.get("hookName") or a.get("hookEvent") or "?"
            conteudo = a.get("content")
            primeira = ""
            if isinstance(conteudo, str):
                primeira = conteudo.strip().split("\n", 1)[0][:_ROTULO_HOOK_CHARS]
            elif isinstance(conteudo, list):
                for b in conteudo:
                    texto = b if isinstance(b, str) else b.get("text") if isinstance(b, dict) else None
                    if isinstance(texto, str) and texto.strip():
                        primeira = texto.strip().split("\n", 1)[0][:_ROTULO_HOOK_CHARS]
                        break
            nome = f"{tipo}:{evento}" + (f" · {primeira}" if primeira else "")
            plugin = plugin_de_hook(primeira)
        else:
            nome = tipo
        self._somar("contexto", nome, plugin=plugin, chamadas=1, ctx_chars=chars)

    def _turno_somar(self, u: dict, ident) -> None:
        """Linhas da mesma resposta repetem o usage CRESCENDO (streaming): vale a última, como
        no leitor de custos. A resposta fica no turno em que apareceu primeiro — um tool_result
        com promptId novo pode chegar no meio dela. Sem identidade, cada linha é uma resposta."""
        chave = ident if ident else ("linha", len(self._respostas))
        turno = self._respostas[chave][0] if chave in self._respostas else len(self._turnos) - 1
        self._respostas[chave] = (turno, (self._dia, self._cwd, self._model,
                                          u.get("speed") == "fast"), u)

    def _turno_tool(self, nome: str, entrada: dict) -> None:
        """Cada tool conta 1 em cada área distinta que tocou."""
        regras = uso_areas.regras_de(self._cwd)
        areas: set[str] = set()
        if nome in ("Read", "Edit", "Write", "NotebookEdit", "Grep", "Glob"):
            caminho = entrada.get("file_path") or entrada.get("notebook_path") or entrada.get("path")
            if isinstance(caminho, str) and caminho:
                areas.add(uso_areas.area_do_caminho(caminho, self._cwd, regras))
        elif nome == "Bash" and isinstance(entrada.get("command"), str):
            areas = uso_areas.areas_do_comando(entrada["command"], self._cwd, regras)
        elif nome == "Skill" and isinstance(entrada.get("skill"), str):
            a = uso_areas.area_do_alvo(f"skill:{entrada['skill']}", regras)
            if a:
                areas.add(a)
        for a in areas:
            self._turnos[-1][a] = self._turnos[-1].get(a, 0) + 1

    def _areas(self) -> None:
        """O uso REAL de cada turno vai às áreas na proporção das tools; sem tool de arquivo, à
        conversa. Tokens por maior resto: a soma das áreas fecha com o turno, sem sobra."""
        somas: dict[int, dict[tuple, dict]] = defaultdict(dict)
        for turno, grupo, u in self._respostas.values():
            t = somas[turno].setdefault(grupo, dict.fromkeys(_CAMPOS_USAGE, 0))
            cw = _int(u.get("cache_creation_input_tokens"))
            t["input_tokens"] += _int(u.get("input_tokens"))
            t["output_tokens"] += _int(u.get("output_tokens"))
            t["cache_creation_input_tokens"] += cw
            t["cache_read_input_tokens"] += _int(u.get("cache_read_input_tokens"))
            criacao = u.get("cache_creation")
            if isinstance(criacao, dict):
                t["cache_1h"] += min(max(0, _int(criacao.get("ephemeral_1h_input_tokens"))), cw)
        for turno, grupos in somas.items():
            contadas = self._turnos[turno]
            pesos = contadas or {uso_areas.CONVERSA: 1}
            for i, ((dia, cwd, model, fast), u) in enumerate(grupos.items()):
                self._dia, self._cwd, self._model = dia, cwd, model
                partes: dict[str, dict] = {a: {} for a in pesos}
                for campo, valor in u.items():
                    for a, v in uso_areas.repartir(valor, pesos).items():
                        partes[a][campo] = v
                for a, p in partes.items():
                    usage = {**p, "cache_creation": {"ephemeral_1h_input_tokens": p["cache_1h"]},
                             "speed": "fast" if fast else ""}
                    # Turno que atravessa dia/modelo: as tools contam só no primeiro grupo.
                    self._somar("area", a, chamadas=contadas.get(a, 0) if i == 0 else 0, usage=usage)

    def resultado(self) -> list[UsoLinha]:
        self._areas()
        return sorted(self._linhas.values(), key=lambda l: (l.dia, l.tipo, l.nome, l.detalhe))
