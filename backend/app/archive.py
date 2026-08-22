"""Arquivo de conversas MORTAS: navega os transcripts .jsonl no disco mesmo sem sessao tmux viva.

O registry so enxerga sessoes tmux; os jsonl antigos ficam orfaos. Aqui: listagem (projeto, preview,
data, se esta em uso por uma sessao viva) + resolucao de path VALIDADA (nunca leitura arbitraria de
disco: projeto no alfabeto do sanitize_cwd, session_id uuid, e o arquivo tem que existir dentro do
projects_dir)."""
import json
import logging
import os
import re
from pathlib import Path
from typing import Optional

from pydantic import BaseModel

from app.config import list_config_dirs, settings
from app.models import ChatEvent
from app.transcript import parse_obj

_log = logging.getLogger("claude_pocket.archive")

_PROJ_RE = re.compile(r"^[A-Za-z0-9-]+$")   # nomes de dir gerados por sanitize_cwd
_SID_RE = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")


class ArchiveFolder(BaseModel):
    # Nivel 1 (pastas): agregado barato — nao abre os transcripts (so 1 leitura de cabecalho por
    # pasta pro cwd real). O preview das conversas so e pago ao ENTRAR na pasta.
    project: str                 # nome do dir em projects/ (cwd sanitizado)
    cwd: Optional[str] = None    # cwd real, lido do transcript mais recente
    count: int                   # quantas conversas na pasta
    mtime: float                 # atividade mais recente


class ArchiveEntry(BaseModel):
    project: str                 # nome do dir em projects/ (cwd sanitizado)
    cwd: Optional[str] = None    # cwd real, lido de dentro do transcript
    session_id: str
    mtime: float
    preview: str                 # 1a msg de usuario (de onde a conversa partiu)
    ultima: str = ""             # ULTIMA msg da conversa -- e o que identifica "qual e essa" meses
                                 # depois; a 1a msg, nao (todas comecam parecidas).
    live: bool = False           # em uso por uma sessao tmux viva agora
    # CONTA dona do transcript (config_dir) e o rotulo dela (conta). Um `claude --resume <uuid>`
    # rodado na conta errada morre na hora com "No conversation found with session ID" -> retomar
    # precisa levar a conta junto, e a tela precisa mostrar de qual conta e cada conversa.
    config_dir: Optional[str] = None
    conta: str = ""
    # Pi, Kimi e Codex guardam transcript FORA do projects/ da conta (ver app.archive_providers).
    # `project` continua sendo o cwd sanitizado, entao a pasta e a mesma pros quatro.
    provider: str = "claude"


def _head_info(jsonl: Path, max_lines: int = 60) -> tuple[str, Optional[str]]:
    # (preview, cwd) lendo so o COMECO do arquivo (early-exit; transcript pode ter dezenas de MB).
    preview: str = ""
    cwd: Optional[str] = None
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for _, line in zip(range(max_lines), fh):
                try:
                    obj = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                if cwd is None:
                    c = obj.get("cwd")
                    if isinstance(c, str) and c:
                        cwd = c
                if not preview:
                    for ev in parse_obj(obj):
                        if ev.kind == "user_msg" and ev.text:
                            preview = _texto_simples(ev.text)[:100]
                            break
                if preview and cwd:
                    break
    except OSError:
        pass
    return preview, cwd


_TAIL_BYTES = 64 * 1024

# Marcacao que o assistente escreve e que numa LINHA de lista so vira ruido (`**titulo**`, crase,
# `#`, `>` e marcador de item). Aqui a saida e uma linha de 100 caracteres, entao renderizar nao e
# opcao: o certo e o texto sem os sinais. Nao e um parser de markdown -- e o suficiente pra prevoa.
_MD_LINK_RE = re.compile(r"\[([^\]]+)\]\([^)]*\)")
_MD_MARCA_RE = re.compile(r"[*_`~]{1,3}")
_MD_INICIO_RE = re.compile(r"^\s{0,3}(?:[#>]+\s*|[-*+]\s+|\d+\.\s+)", re.MULTILINE)


def _texto_simples(txt: str) -> str:
    txt = _MD_LINK_RE.sub(r"\1", txt)
    txt = _MD_INICIO_RE.sub("", txt)
    txt = _MD_MARCA_RE.sub("", txt)
    return " ".join(txt.split())


def _linhas_do_fim(jsonl: Path, span: int) -> tuple[list[bytes], bool]:
    """(linhas do fim do arquivo, pegou_o_inicio). Seek, nao varredura: o transcript pode ter dezenas
    de MB. A 1a linha e descartada quando o seek caiu no meio dela."""
    try:
        with open(jsonl, "rb") as fh:
            tam = fh.seek(0, os.SEEK_END)
            inicio = max(0, tam - span)
            fh.seek(inicio)
            bruto = fh.read()
    except OSError:
        # Nao levanta: a lista inteira cairia por causa de UM transcript. Mas registra — sem isto,
        # falha de leitura vira "(sem mensagens)" na tela, igualzinho a conversa vazia de verdade.
        _log.warning("arquivo: nao consegui ler o fim de %s", jsonl, exc_info=True)
        return [], True
    linhas = bruto.split(b"\n")
    if inicio > 0:
        return linhas[1:], False
    return linhas, True


def _obj_da_linha(linha: bytes) -> Optional[dict]:
    if not linha.strip():
        return None
    try:
        obj = json.loads(linha.decode("utf-8", errors="replace"))
    except (json.JSONDecodeError, ValueError):
        return None
    return obj if isinstance(obj, dict) else None


def _parse(provider: str, obj: dict) -> list[ChatEvent]:
    if provider == "claude":
        return parse_obj(obj)
    from app import archive_providers
    return archive_providers.parse_obj(provider, obj)


def _tail_info(jsonl: Path, provider: str = "claude") -> str:
    """Ultima msg (de quem for) da conversa. Duas passadas de tamanho crescente porque UMA entrada
    pode passar de 64KB sozinha (imagem colada, saida grande de ferramenta)."""
    for span in (_TAIL_BYTES, _TAIL_BYTES * 16):
        linhas, do_inicio = _linhas_do_fim(jsonl, span)
        for linha in reversed(linhas):
            obj = _obj_da_linha(linha)
            if obj is None:
                continue
            for ev in reversed(_parse(provider, obj)):
                if ev.kind in ("user_msg", "assistant_msg") and ev.text:
                    return _texto_simples(ev.text)[:100]
        if do_inicio:
            break   # ja era o arquivo inteiro: aumentar o span nao traz mais nada
    return ""


def _folder_files(proj: Path) -> list[tuple[float, Path]]:
    files: list[tuple[float, Path]] = []
    for f in proj.glob("*.jsonl"):
        try:
            files.append((f.stat().st_mtime, f))
        except OSError:
            continue
    files.sort(key=lambda t: t[0], reverse=True)
    return files


def _base(config_dir: Optional[str] = None) -> Path:
    """O projects/ da CONTA pedida. None = a conta do processo do backend.
    O caminho nao e validado aqui: quem aceita config_dir vindo do cliente confere antes contra
    config.list_config_dirs(), a mesma guarda do POST /api/sessions."""
    return (Path(config_dir) / "projects") if config_dir else Path(settings.projects_dir)


def _contas(config_dir: Optional[str] = None) -> list[tuple[Optional[str], str, Path]]:
    """(config_dir, rotulo, projects/) das contas a varrer. Com `config_dir`, so aquela. Sem ele,
    TODAS -- conversa de outra conta e invisivel de outro jeito, e foi assim que uma conversa viva
    em ~/.claude-<outra> simplesmente nao aparecia no Arquivo.

    A conta do processo entra por ultimo e so se ainda nao veio pela lista: ela costuma estar nas
    duas, e a entrada da lista e melhor (traz o rotulo que o usuario batizou na aba Contas)."""
    if config_dir:
        # O rotulo vem da lista mesmo com filtro: devolver "" aqui fazia a conta perder o nome na
        # tela exatamente quando o usuario ja tinha escolhido uma.
        rotulo = next((c.label for c in list_config_dirs() if c.path == config_dir), "")
        return [(config_dir, rotulo, _base(config_dir))]
    out: list[tuple[Optional[str], str, Path]] = []
    vistos: set[Path] = set()
    for c in list_config_dirs():
        p = Path(c.path) / "projects"
        try:
            r = p.resolve()
        except OSError:
            continue
        if r in vistos:
            continue
        vistos.add(r)
        out.append((c.path, c.label, p))
    propria = Path(settings.projects_dir)
    try:
        if propria.resolve() not in vistos:
            out.append((None, "", propria))
    except OSError:
        pass
    return out


def _projeto_de(cwd: Optional[str]) -> Optional[str]:
    """Nome de pasta (project) a partir do cwd real -- a MESMA regra do Claude, pra Pi/Kimi/Codex
    caírem na pasta ja existente em vez de criar uma paralela. Import local: registry puxa tmux e
    os adapters, e o archive e importado por caminhos que nao precisam disso."""
    if not cwd:
        return None
    from app.registry import sanitize_cwd
    proj = sanitize_cwd(cwd)
    return proj if _PROJ_RE.match(proj) else None


def _conversas_de_outros_providers():
    from app import archive_providers
    return archive_providers.conversas()


def list_folders(config_dir: Optional[str] = None) -> list[ArchiveFolder]:
    """Nivel 1: as pastas (projetos) com conversa arquivada, mais recentes primeiro. A MESMA pasta
    costuma existir em varias contas -- aqui elas somam numa linha so (a pasta e o que o usuario
    procura; de qual conta e cada conversa so importa um nivel abaixo)."""
    agg: dict[str, ArchiveFolder] = {}
    for _cfg, _rot, base in _contas(config_dir):
        try:
            projdirs = [d for d in base.iterdir() if d.is_dir()]
        except OSError:
            continue
        for proj in projdirs:
            files = _folder_files(proj)
            if not files:
                continue
            _, cwd = _head_info(files[0][1])   # cwd real do transcript mais recente (1 leitura/pasta)
            ja = agg.get(proj.name)
            if ja is None:
                agg[proj.name] = ArchiveFolder(project=proj.name, cwd=cwd, count=len(files),
                                               mtime=files[0][0])
                continue
            ja.count += len(files)
            if files[0][0] > ja.mtime:
                ja.mtime, ja.cwd = files[0][0], cwd or ja.cwd
    # Pi, Kimi e Codex entram na MESMA pasta do Claude: `project` e o cwd sanitizado nos quatro, e
    # o usuario procura a pasta, nao o agente. Filtrar por conta nao se aplica -- nenhum dos tres
    # tem conta Claude —, entao com config_dir pedido eles ficam de fora.
    if not config_dir:
        for c in _conversas_de_outros_providers():
            proj = _projeto_de(c.cwd)
            if proj is None:
                continue
            ja = agg.get(proj)
            if ja is None:
                agg[proj] = ArchiveFolder(project=proj, cwd=c.cwd, count=1, mtime=c.mtime)
                continue
            ja.count += 1
            if c.mtime > ja.mtime:
                ja.mtime, ja.cwd = c.mtime, c.cwd or ja.cwd
    out = list(agg.values())
    out.sort(key=lambda e: e.mtime, reverse=True)
    return out


def list_conversations(project: str, live_realpaths: set[str], cap: int = 100,
                       config_dir: Optional[str] = None) -> list[ArchiveEntry]:
    """Nivel 2: conversas de UMA pasta, em TODAS as contas (ou so na pedida), mais recentes
    primeiro. O preview abre cada arquivo -> o teto limita o custo por request; ele vale pro
    resultado JUNTO, senao 4 contas custariam 4x o teto. FileNotFoundError so quando a pasta nao
    existe em conta nenhuma."""
    if not _PROJ_RE.match(project):
        raise ValueError("caminho invalido")
    achou = False
    linhas: list[tuple[float, Path, Optional[str], str]] = []
    for cfg, rotulo, base in _contas(config_dir):
        proj = base / project
        if not proj.is_dir():
            continue
        achou = True
        linhas += [(mt, f, cfg, rotulo) for mt, f in _folder_files(proj)]
    # Ordenar ANTES do teto: a ordem que vem do disco e a do glob/indice, nao a de recencia, entao
    # cortar direto descartaria conversa NOVA e manteria velha -- calado, e justo na lista que diz
    # "mais recentes primeiro".
    outras = sorted((c for c in (() if config_dir else _conversas_de_outros_providers())
                     if _projeto_de(c.cwd) == project),
                    key=lambda c: c.mtime, reverse=True)
    if not achou and not outras:
        raise FileNotFoundError(project)
    linhas.sort(key=lambda t: t[0], reverse=True)
    out: list[ArchiveEntry] = []
    for mt, f, cfg, rotulo in linhas[:cap]:
        preview, cwd = _head_info(f)
        out.append(ArchiveEntry(
            project=project, cwd=cwd, session_id=f.stem, mtime=mt,
            preview=preview, ultima=_tail_info(f), config_dir=cfg, conta=rotulo,
            live=os.path.realpath(str(f)) in live_realpaths,
        ))
    for c in outras[:cap]:
        # `preview` fica vazio de proposito: nos outros providers o cwd ja veio do indice/cabecalho,
        # entao abrir o comeco do arquivo so pra pegar a 1a msg seria uma leitura a mais por uma
        # informacao que a lista nem mostra (quem identifica a conversa e a ULTIMA msg).
        out.append(ArchiveEntry(
            project=project, cwd=c.cwd, session_id=c.session_id, mtime=c.mtime,
            preview="", ultima=_tail_info(c.path, c.provider), provider=c.provider,
            live=os.path.realpath(str(c.path)) in live_realpaths,
        ))
    out.sort(key=lambda e: e.mtime, reverse=True)
    return out[:cap]


def archive_jsonl(project: str, session_id: str, config_dir: Optional[str] = None,
                  provider: str = "claude") -> Path:
    """Path validado do transcript arquivado. ValueError = componente invalido (traversal barrado);
    FileNotFoundError = nao existe.

    Sem `config_dir`, procura em TODAS as contas e a 1a que tiver o arquivo vence -- o uuid e unico,
    entao nao ha ambiguidade a resolver. E o que faz um link direto pra conversa (que so carrega
    projeto e uuid) continuar abrindo quando ela e de outra conta.

    Fora do Claude quem resolve e o archive_providers: o caminho nao sai do (project, session_id),
    porque o nome do arquivo do Pi e do Codex carrega um timestamp que nao da pra recriar."""
    if provider != "claude":
        from app import archive_providers
        return archive_providers.jsonl_de(provider, session_id)
    if not _PROJ_RE.match(project) or not _SID_RE.match(session_id):
        raise ValueError("caminho invalido")
    ultimo: Optional[Path] = None
    for _cfg, _rot, base in _contas(config_dir):
        p = base / project / f"{session_id}.jsonl"
        ultimo = p
        if p.is_file():
            return p
    raise FileNotFoundError(str(ultimo or (_base(config_dir) / project / f"{session_id}.jsonl")))


def tail_events(project: str, session_id: str, n: int = 30, config_dir: Optional[str] = None,
                provider: str = "claude") -> list[ChatEvent]:
    """As ULTIMAS n mensagens da conversa, pra confirmar "e essa mesmo?" antes de retomar. Le pelo
    fim (`seek`), NAO pelo /history: aquele carrega o transcript inteiro, e aqui um arquivo de 19MB
    seria aberto so pra mostrar cinco balões. So user/assistente -- chamada de ferramenta encheria a
    caixa de ruido justamente onde o espaco e pouco.

    O span cresce ate juntar n eventos ou ate o arquivo acabar; conversa curta devolve menos que n.
    Crescer e obrigatorio, nao otimizacao: medido nesta sessao, as ultimas 2MB de um transcript de
    15MB tinham 300 linhas e apenas SEIS mensagens de texto -- o resto era `attachment`, tool_use e
    metadado. Com um span fixo a previa vinha quase vazia justo nas conversas longas."""
    p = archive_jsonl(project, session_id, config_dir, provider)
    out: list[ChatEvent] = []
    span = _TAIL_BYTES * 4
    while True:
        linhas, do_inicio = _linhas_do_fim(p, span)
        out = []
        for linha in reversed(linhas):
            obj = _obj_da_linha(linha)
            if obj is None:
                continue
            for ev in reversed(_parse(provider, obj)):
                if ev.kind in ("user_msg", "assistant_msg") and ev.text:
                    out.append(ev)
            if len(out) >= n:
                break
        if len(out) >= n or do_inicio:
            return list(reversed(out[:n]))
        span *= 8


def conta_de(project: str, session_id: str) -> Optional[str]:
    """config_dir da conta que guarda esta conversa (None = a do processo do backend). Quem retoma
    precisa dele: `claude --resume <uuid>` rodado na conta errada morre na hora. Mesma validacao e
    mesmos erros de archive_jsonl."""
    if not _PROJ_RE.match(project) or not _SID_RE.match(session_id):
        raise ValueError("caminho invalido")
    for cfg, _rot, base in _contas():
        if (base / project / f"{session_id}.jsonl").is_file():
            return cfg
    raise FileNotFoundError(project)


def archive_cwd(project: str, session_id: str, config_dir: Optional[str] = None,
                provider: str = "claude") -> Optional[str]:
    """cwd real da conversa arquivada (lido do cabecalho do transcript) -- usado pra retomar (feature
    'Retomar conversa'): a sessao tmux nova precisa nascer no MESMO cwd da conversa original. Mesma
    validacao de archive_jsonl (propaga ValueError/FileNotFoundError); None = cwd nao ficou gravado
    nas primeiras linhas do transcript (conversa nao pode ser retomada)."""
    p = archive_jsonl(project, session_id, config_dir, provider)
    if provider == "claude":
        _, cwd = _head_info(p)
        return cwd
    # Pi e Codex gravam o cwd num cabecalho de formato proprio; o Kimi nem grava (vem do indice).
    from app import archive_providers
    for c in archive_providers.conversas():
        if c.provider == provider and c.session_id == session_id:
            return c.cwd
    return None
