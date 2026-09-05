"""Progresso do plano do superpowers que uma sessao esta executando.

Le o .md do plano no repo do cwd da sessao e conta os steps marcados. Roda por sessao a cada poll
da lista, entao TUDO aqui e cacheado e NADA levanta: o unico consumidor e o tick que alimenta o
SSE, e uma excecao ali derruba a lista inteira (incidente de 2026-07-23 com o git status).

Stdlib apenas — sem import de app.config, mesmo motivo do engines.py.
"""

from __future__ import annotations

import logging
import os
import re
import threading
import time
from dataclasses import dataclass
from pathlib import Path

from app import atomico

_log = logging.getLogger("hangar.planprog")

PLANS_REL = os.path.join("docs", "superpowers", "plans")

# Onde o plano encerrado vai parar. Subpasta da propria pasta de planos: o _discover so olha os .md
# do primeiro nivel (`e.is_file()`), entao mover pra ca ja o tira da eleicao, do seletor e da barra
# sem apagar nada. Era convencao manual do usuario (56 arquivos aqui) — virou botao.
FEITOS_REL = "feitos"

# Quantos niveis subir a partir do cwd da pane procurando a raiz do repo. `#{pane_current_path}`
# segue o `cd` do usuario, entao um `cd frontend/src` nao pode fazer o plano sumir da UI. A subida
# PARA no primeiro diretorio com .git: sem isso, um worktree em .claude/worktrees/<x> sem planos
# subiria ate o checkout principal e mostraria o plano de OUTRO trabalho. "Sem barra" e limitacao;
# "barra errada" e bug.
_MAX_PARENTS = 6

# Plano parado ha mais de 14 dias nao reaparece.
# ponytail: mtime nao mede abandono de verdade — git checkout/worktree add reescrevem o mtime de
# todos os planos. Isto so evita que um plano de meses atras ressuscite; nao promete mais que isso.
_MAX_AGE_S = 14 * 86400

# UM regex pros dois usos (descoberta e parse). Quando a descoberta procurava "- [x]" solto e o
# parse so contava Step, um checkbox de checklist elegia um plano que depois lia 0/N e escondia o
# plano real.
_STEP_RE = re.compile(r"^- \[([ xX])\] \*\*(Step\b[^*]*)\*\*", re.M)
_TASK_RE = re.compile(r"^### (Task\b[^\n]*)$", re.M)
# Bloco cercado (``` ou ~~~). Planos MOSTRAM steps de exemplo dentro de bloco de codigo — sem tirar
# isto, um plano recem-escrito ja nasce com "3/47 feitos" e acende a barra antes de comecar
# (medido no proprio plano deste trabalho: 47 casados vs 42 reais, 3 "marcados" vs 0).
_FENCE_RE = re.compile(r"^(```|~~~).*?^\1[^\n]*$", re.M | re.S)
_MANUAL_RE = re.compile(r"verifica(?:ção|cao|r)\s+manual|manual\s+verification", re.I)
_DATE_PREFIX_RE = re.compile(r"^\d{4}-\d{2}-\d{2}-")


@dataclass(frozen=True)
class StepProgress:
    title: str
    done: bool
    manual: bool
    # Posicao 0-based na ordem do DOCUMENTO (nao dentro da Task). E a chave que o cliente devolve
    # pra marcar/desmarcar: titulo repete entre Tasks e o par (task, step) exigiria refazer o
    # recorte de Task no caminho de escrita.
    idx: int


@dataclass(frozen=True)
class TaskProgress:
    title: str
    done: int
    total: int
    steps: tuple[StepProgress, ...]


@dataclass(frozen=True)
class PlanProgress:
    name: str
    path: str
    task_idx: int      # posicao ORDINAL (1-based) da 1a Task com step pendente
    task_total: int
    done: int
    total: int
    complete: bool
    tasks: tuple[TaskProgress, ...]


# path -> (mtime_ns, PlanProgress | None). O None memoriza "li e nao serve" — e o que impede reler
# os candidatos sem marcacao a cada poll (neste repo, ~2.5k linhas por sessao por poll).
_file_cache: dict[str, tuple[int, PlanProgress | None]] = {}
# raiz -> (ts, path | None). Mesmo padrao do _summary_cache do git_ops: com N sessoes no mesmo repo,
# o scandir roda 1x e nao N.
_discovery_cache: dict[str, tuple[float, str | None]] = {}
_DISCOVERY_TTL = 3.0
# raiz -> path do plano eleito no ciclo anterior. Enquanto ele tiver step pendente, continua eleito:
# sem isto, o writing-plans reescrevendo OUTRO plano rouba o posto e o progresso pula 9/17 -> 3/56.
_sticky: dict[str, str] = {}


# Pin manual: o usuario escolhe QUAL plano o painel mostra, em vez de aceitar o eleito. Guardado por
# RAIZ DE PLANOS (nao por sessao): duas sessoes no mesmo repo trabalham o mesmo plano, e e assim que
# o _discover ja e chaveado. Arquivo ao lado dos planos, no repo — segue o repo, nao o navegador.
PIN_FILE = "cp-plan-pin"

# "nao quero plano nenhum" e um pin como os outros, nao um terceiro arquivo: e a mesma pergunta
# ("qual plano este repo mostra?") com a resposta "nenhum". O `!` nao pode abrir um stem valido do
# seletor (os planos sao AAAA-MM-DD-*), entao nao ha como um .md real colidir com o sentinela.
PIN_NONE = "!none"


def _pin_path(root: str) -> str:
    """Dentro do `.git/` do repo dono da pasta de planos. Motivo: `.git/` nunca e rastreado, entao o
    pin nao aparece no `git status` de quem versiona os planos — e some junto com o clone, que e o
    comportamento certo pra uma preferencia local. `.git` como ARQUIVO (worktree) nao serve de
    diretorio: nesse caso cai na propria pasta de planos, com ponto na frente."""
    cur = Path(root)
    for _ in range(_MAX_PARENTS + 1):
        g = cur / ".git"
        if g.is_dir():
            return str(g / PIN_FILE)
        if cur.parent == cur:
            break
        cur = cur.parent
    return os.path.join(root, "." + PIN_FILE)


def is_safe_stem(v: str) -> bool:
    """Guarda anti-traversal: o valor vira nome de arquivo. So o stem, sem separador. Vale pra
    TODA entrada de stem — a do disco (read_pin) e a que vem do cliente (endpoint do pin)."""
    return bool(v) and "/" not in v and "\\" not in v and v not in (".", "..")


def read_pin(root: str) -> str | None:
    """Stem do plano fixado nesta raiz, ou None. NUNCA levanta — pin ilegivel = sem pin.

    Pin apontando pra .md que nao existe mais tambem e sem pin. Sem esta conferencia o arquivo
    sobrevive ao plano (apagado, renomeado, movido pra `feitos/`) e o painel passa a MENTIR: o
    `plan_progress` ja caia na eleicao automatica, mas o `list_plans` devolvia o stem morto e o
    seletor o exibia como rotulo — titulo de um plano, barra e Tasks de outro. Medido em
    27/08/2026 neste repo: o rotulo era o nome de um plano de 04/08 que nao existe mais em
    `plans/`, e a barra embaixo dele contava os steps do `painel-orquestracao`."""
    try:
        v = Path(_pin_path(root)).read_text(encoding="utf-8", errors="replace").strip()
    except OSError:
        return None
    if not is_safe_stem(v):
        return None
    if v == PIN_NONE:
        return v      # sentinela: nao tem arquivo pra conferir
    return v if os.path.isfile(os.path.join(root, v + ".md")) else None


def write_pin(root: str, stem: str | None) -> None:
    """Fixa (ou solta, com None) o plano da raiz. Solta tambem o sticky, senao o eleito anterior
    continuaria mandando ate ganhar/perder pendencia por conta propria."""
    p = _pin_path(root)
    try:
        if stem is None:
            Path(p).unlink(missing_ok=True)
        else:
            Path(p).write_text(stem + "\n", encoding="utf-8")
    except OSError as e:
        raise PlanPinError(str(e))
    _discovery_cache.pop(root, None)
    _sticky.pop(root, None)


class PlanPinError(Exception):
    pass


class PlanWriteError(Exception):
    """Falha ao ESCREVER no plano (marcar step, arquivar). Separada da PlanPinError porque aqui o
    arquivo do usuario esta em jogo — quem trata devolve 409/500, nunca engole."""


def caminho_do_plano(root: str, stem: str) -> str:
    """Path do .md de `stem` dentro de `root`. Levanta PlanWriteError pra stem com separador — a
    unica coisa que impede um nome vindo do cliente de virar caminho pra fora da pasta."""
    if not is_safe_stem(stem):
        raise PlanWriteError(f"nome de plano invalido: {stem}")
    return os.path.join(root, stem + ".md")


# Serializa o ciclo ler-alterar-gravar do `marcar_step`. Duas marcacoes quase simultaneas — o app
# roda no celular E no desktop ao mesmo tempo, na mesma sessao — liam o MESMO `raw` antes de
# qualquer uma gravar, e a segunda regravava o arquivo inteiro por cima, desfazendo a primeira em
# silencio (a checagem de `raw[pos]` so olha o byte do proprio idx, entao ela nao pega isso).
# ponytail: um lock global pro backend inteiro. A secao critica e ler um .md e trocar um caractere;
# se um dia isto virar gargalo, o passo seguinte e um lock por caminho.
_lock_escrita = threading.Lock()


def marcar_step(path: str, idx: int, done: bool) -> None:
    """Marca (ou desmarca) o step de indice `idx` — 0-based, na ordem do documento — do plano.

    Existe porque quem marca o `- [x]` e o agente, e quando ele esquece o plano trava: fica em
    14/16 pra sempre, nunca fecha, nunca sai do painel. Aqui a pessoa fecha na mao.

    Escreve UM caractere: o resto do arquivo sai byte a byte igual. Nada de reserializar markdown —
    o plano e do usuario e esta versionado, um reformat viraria diff que ninguem pediu.
    """
    with _lock_escrita:
        try:
            raw = Path(path).read_bytes().decode("utf-8")
        except OSError as e:
            raise PlanWriteError(str(e))
        except UnicodeDecodeError:
            # Os outros caminhos leem com errors="replace" porque so exibem. Aqui a leitura volta
            # pro disco: gravar o U+FFFD apagaria o byte original do arquivo do usuario.
            raise PlanWriteError("o plano nao esta em UTF-8")

        # Mesma neutralizacao de bloco cercado do parse, PRESERVANDO offsets — e o que deixa casar
        # no texto limpo e escrever no original. Sem ela, um step de exemplo dentro de ``` entraria
        # na contagem e o indice apontaria pro checkbox errado.
        limpo = _FENCE_RE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), raw)
        ms = list(_STEP_RE.finditer(limpo))
        if idx < 0 or idx >= len(ms):
            raise PlanWriteError(f"step {idx} nao existe (o plano tem {len(ms)})")
        pos = ms[idx].start(1)
        if raw[pos] not in " xX":
            # So acontece se o arquivo mudou entre a leitura que gerou a tela e este clique. Recusa
            # explicita: escrever no offset errado corromperia o texto do plano.
            raise PlanWriteError("o plano mudou no disco — recarregue antes de marcar")

        novo = raw[:pos] + ("x" if done else " ") + raw[pos + 1:]
        # pid no nome separa PROCESSOS; duas marcacoes do mesmo backend cairiam no mesmo temporario
        # (medido: a segunda achava o arquivo ja renomeado e levantava FileNotFoundError). Quem
        # separa as duas e o `_lock_escrita` acima — o pid e a rede pro caso de outro processo.
        tmp = f"{path}.{os.getpid()}.tmp"
        try:
            Path(tmp).write_text(novo, encoding="utf-8")
            atomico.substituir(tmp, path)
        except OSError as e:
            Path(tmp).unlink(missing_ok=True)
            raise PlanWriteError(str(e))
    # _file_cache e chaveado por mtime_ns, entao a proxima leitura ja reparseia sozinha.


def arquivar(root: str, stem: str) -> list[str]:
    """Move `<stem>.md` (e o `.html` irmao, quando existe) pra `<root>/feitos/`. Devolve o que moveu.

    Mover, nao apagar: o plano e o registro do que foi feito e esta versionado. E move o .html
    junto porque o par nasce junto — deixar o irmao orfao na pasta e exatamente o lixo que isto
    veio limpar.
    """
    origem_md = caminho_do_plano(root, stem)
    if not os.path.isfile(origem_md):
        raise PlanWriteError(f"plano nao encontrado: {stem}")

    # Antes de mover: depois, o read_pin ja nao reconhece o stem (o .md sumiu) e o pin morto ficaria
    # no disco pra sempre.
    pin_era_este = read_pin(root) == stem

    destino = os.path.join(root, FEITOS_REL)
    movidos: list[str] = []
    try:
        os.makedirs(destino, exist_ok=True)
        a_mover = [stem + ext for ext in (".md", ".html")
                   if os.path.isfile(os.path.join(root, stem + ext))]
        # TUDO ou NADA: a colisao dos DOIS destinos e conferida ANTES de mover qualquer um. Dentro
        # do laco, um `.html` orfao ja em `feitos/` (sobra de um arquivamento anterior) so era
        # notado depois do `.md` ja ter saido — o plano sumia da pasta ativa e a tela ainda dizia
        # "nao deu pra arquivar", que e o contrario do que tinha acontecido.
        for nome in a_mover:
            if os.path.exists(os.path.join(destino, nome)):
                # NUNCA sobrescreve: o de la e um plano encerrado de verdade, com o mesmo nome.
                raise PlanWriteError(f"ja existe {nome} em {FEITOS_REL}/")
        for nome in a_mover:
            try:
                atomico.substituir(os.path.join(root, nome), os.path.join(destino, nome))
            except OSError as e:
                # Sobrou o caso improvavel (disco cheio, permissao) DEPOIS do primeiro ter movido.
                # A mensagem diz o que ja saiu: sem isso a pessoa le "nao deu pra arquivar", tenta
                # de novo e recebe "plano nao encontrado", sem nunca saber que metade se mexeu.
                if movidos:
                    raise PlanWriteError(f"moveu {', '.join(movidos)} e falhou em {nome}: {e}")
                raise PlanWriteError(str(e))
            movidos.append(nome)
    except OSError as e:
        raise PlanWriteError(str(e))
    finally:
        # Fora do try de sucesso: num arquivamento parcial o `.md` JA saiu, e um cache apontando
        # pro caminho velho faria a proxima leitura descrever um estado que nao existe mais.
        _file_cache.pop(origem_md, None)
        _file_cache.pop(origem_md + "\x00pin", None)
        _discovery_cache.pop(root, None)
        _sticky.pop(root, None)

    if pin_era_este:
        try:
            write_pin(root, None)
        except PlanPinError:
            # O pin morto agora e inofensivo (read_pin o descarta), entao nao vale falhar o
            # arquivamento que ja aconteceu — mas nao some calado.
            _log.warning("plano arquivado, mas nao deu pra soltar o pin root=%s", root, exc_info=True)
    return movidos


def _reset_caches() -> None:
    """So pra teste."""
    _file_cache.clear()
    _discovery_cache.clear()
    _sticky.clear()


def _invalidate_discovery() -> None:
    """So pra teste: simula o TTL da descoberta vencendo, sem sleep de 3s."""
    _discovery_cache.clear()


def _plans_dir(cwd: str) -> str | None:
    """Sobe ate _MAX_PARENTS niveis procurando docs/superpowers/plans, PARANDO no primeiro nivel que
    tenha .git (raiz do repo ou do worktree)."""
    cur = Path(cwd)
    for _ in range(_MAX_PARENTS + 1):
        cand = cur / PLANS_REL
        if cand.is_dir():
            return str(cand)
        if (cur / ".git").exists():
            return None        # raiz do repo alcancada e sem planos: nao vaza pro repo de fora
        if cur.parent == cur:
            break
        cur = cur.parent
    return None


def parse_plan(path: str, require_started: bool = True) -> PlanProgress | None:
    """Parseia UM plano. None se nao tem step nenhum ou — com require_started — nenhum step marcado.
    Pode levantar OSError (quem chama trata) — so o I/O levanta; markdown nao falha ao parsear.

    `require_started=False` e pro plano FIXADO na mao: a regra de descartar plano nao-comecado existe
    pra ele nao acender a barra sozinho, e quando o usuario escolhe explicitamente ela vira estorvo —
    fixa-se um plano justamente porque se vai comecar ele."""
    # read_bytes de uma vez (nao linha a linha): o Edit trunca e reescreve, e um poll no meio da
    # escrita leria menos steps -> a sig cai e sobe = piscada em todas as views ao mesmo tempo.
    raw = Path(path).read_bytes().decode("utf-8", errors="replace")
    # Neutraliza os blocos de codigo PRESERVANDO offsets (troca cada char por espaco, menos \n):
    # as fronteiras de Task sao calculadas por posicao, entao remover texto quebraria o recorte.
    raw = _FENCE_RE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), raw)

    steps = [(m.start(), m.group(1) != " ", m.group(2).strip(), i)
             for i, m in enumerate(_STEP_RE.finditer(raw))]
    if not steps:
        return None
    done = sum(1 for _, ok, _, _ in steps if ok)
    if done == 0 and require_started:
        return None   # escrito mas nunca comecado: nao acende barra SOZINHO (pin passa por cima)

    heads = [(m.start(), m.group(1).strip()) for m in _TASK_RE.finditer(raw)]
    if not heads:
        heads = [(0, "Task 1")]
    elif heads[0][0] > 0 and any(s[0] < heads[0][0] for s in steps):
        # Steps antes da 1a Task (ex.: checklist solto no topo) entram no done/total geral pelo
        # scan do documento inteiro (linha 125); sem esta Task implicita eles somem da lista de
        # Tasks e sum(t.done/total) nunca bate com r.done/r.total (o que alimenta a barra
        # segmentada plan_tasks).
        heads = [(0, "(sem Task)")] + heads

    tasks: list[TaskProgress] = []
    for i, (pos, title) in enumerate(heads):
        end = heads[i + 1][0] if i + 1 < len(heads) else len(raw)
        mine = [s for s in steps if pos <= s[0] < end]
        tasks.append(TaskProgress(
            title=title,
            done=sum(1 for _, ok, _, _ in mine if ok),
            total=len(mine),
            steps=tuple(StepProgress(title=t, done=ok, manual=bool(_MANUAL_RE.search(t)), idx=i)
                        for _, ok, t, i in mine),
        ))

    # ORDINAL, nao o N do heading: existe "### Task 0" nos planos (pi-adapter), e "Task 0/6" no chip
    # seria mentira. O painel mostra o titulo literal.
    task_idx = next((i + 1 for i, t in enumerate(tasks) if t.done < t.total), len(tasks))
    name = _DATE_PREFIX_RE.sub("", Path(path).stem)
    return PlanProgress(name=name, path=path, task_idx=task_idx, task_total=len(tasks),
                        done=done, total=len(steps), complete=done == len(steps),
                        tasks=tuple(tasks))


def _load(path: str, mtime_ns: int, require_started: bool = True) -> PlanProgress | None:
    # A chave do cache carrega o require_started: o MESMO arquivo tem dois resultados possiveis
    # (None pela eleicao automatica, PlanProgress pelo pin), e uma chave so faria um envenenar o
    # outro conforme quem lesse primeiro.
    key = path if require_started else path + "\x00pin"
    hit = _file_cache.get(key)
    if hit is not None and hit[0] == mtime_ns:
        return hit[1]
    got = parse_plan(path, require_started)
    _file_cache[key] = (mtime_ns, got)
    return got


def _discover(root: str) -> str | None:
    """Path do plano ativo em `root` (dir de planos), ou None. Preferencia: plano com step pendente;
    entre eles, o mtime mais novo. Sticky: o eleito anterior mantem o posto enquanto tiver pendencia."""
    now_wall = time.time()
    cands: list[tuple[float, str, PlanProgress]] = []
    with os.scandir(root) as it:
        for e in it:
            if not e.name.endswith(".md") or not e.is_file():
                continue
            try:
                st = e.stat()
                if now_wall - st.st_mtime > _MAX_AGE_S:
                    continue
                got = _load(e.path, st.st_mtime_ns)
            except OSError as e:
                # UM arquivo ilegivel nao pode matar a feature do repo inteiro: sem este continue,
                # a excecao subiria ao except de plan_progress e devolveria None pra TODAS as
                # sessoes daquele repo, a cada poll, pra sempre.
                _log.warning("plano ilegivel path=%s", e.filename or e, exc_info=True)
                continue
            if got is not None:
                cands.append((st.st_mtime, e.path, got))
    if not cands:
        return None

    prev = _sticky.get(root)
    for _, path, got in cands:
        if path == prev and not got.complete:
            return path        # sticky: quem esta andando nao perde o posto

    pend = [c for c in cands if not c[2].complete]
    pool = pend or cands
    pool.sort(key=lambda c: c[0], reverse=True)      # mais NOVO primeiro
    chosen = pool[0][1]
    _sticky[root] = chosen
    return chosen


def list_plans(cwd: str | None) -> dict | None:
    """Todos os planos da raiz do repo em `cwd`, pro seletor — inclusive os que NAO acendem a barra
    (zero step marcado) e os completos, que o _discover descarta. Aqui a lista e pra escolher, e
    esconder opcao seria justamente o que fez o usuario nao conseguir trocar. None = repo sem pasta
    de planos. NUNCA levanta."""
    try:
        if not cwd:
            return None
        root = _plans_dir(cwd)
        if root is None:
            return None
        itens = []
        with os.scandir(root) as it:
            for e in it:
                if not e.name.endswith(".md") or not e.is_file():
                    continue
                stem = e.name[:-3]
                try:
                    got = _load(e.path, e.stat().st_mtime_ns)
                except OSError as err:
                    # Mesmo motivo do _discover: um arquivo ilegivel nao derruba a lista inteira.
                    # Mas some do seletor sem deixar rastro — sem o log e indistinguivel de "nao
                    # existe", e ninguem investiga o que nunca apareceu.
                    _log.warning("plano ilegivel path=%s", err.filename or err, exc_info=True)
                    continue
                itens.append({
                    "stem": stem,
                    "name": _DATE_PREFIX_RE.sub("", stem),
                    "done": got.done if got else 0,
                    "total": got.total if got else _count_steps(e.path),
                    "complete": bool(got and got.complete),
                })
        itens.sort(key=lambda p: p["stem"], reverse=True)   # mais recente primeiro (prefixo de data)
        return {"plans": itens, "pinned": read_pin(root)}
    except Exception:
        _log.warning("list_plans falhou pra cwd=%r", cwd, exc_info=True)
        return None


def _count_steps(path: str) -> int:
    """Total de steps de um plano que o _load descartou (zero marcado). So pra mostrar '0/28' no
    seletor em vez de '0/0' — sem isto, plano nao comecado parece nao ter passo nenhum."""
    try:
        raw = Path(path).read_bytes().decode("utf-8", errors="replace")
    except OSError:
        return 0
    raw = _FENCE_RE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), raw)
    return len(_STEP_RE.findall(raw))


def plano_escondido(cwd: str | None) -> bool:
    """O repo em `cwd` esta com o pin sentinela (PIN_NONE) ligado. NUNCA levanta.

    Existe porque `plan_progress` devolve None tanto pra "nao ha plano" quanto pra "o usuario
    escondeu" — e a UI PRECISA distinguir: o seletor que desfaz a escolha mora no painel do plano,
    e o painel so e montado quando ha `plan_name`. Sem este sinal, escolher "nenhum" desmontava o
    proprio controle de voltar, e a unica saida era apagar `.git/cp-plan-pin` na mao."""
    try:
        root = _plans_dir(cwd) if cwd else None
        return root is not None and read_pin(root) == PIN_NONE
    except Exception:
        # Loga como as vizinhas: read_pin ja nao propaga, entao chegar aqui e bug de verdade — e o
        # False silencioso reproduz EXATAMENTE o sintoma que esta funcao existe pra impedir (painel
        # some e a unica saida vira apagar o pin na mao), sem deixar rastro pra quem for investigar.
        _log.warning("plano_escondido falhou pra cwd=%r", cwd, exc_info=True)
        return False


def plan_progress(cwd: str | None) -> PlanProgress | None:
    """Progresso do plano ativo do repo em `cwd`, ou None. NUNCA levanta."""
    try:
        if not cwd:
            return None
        root = _plans_dir(cwd)
        if root is None:
            return None
        # Pin manual vence a eleicao — MAS so enquanto o plano fixado tiver step pendente. Ao chegar
        # em 100% ele volta pro automatico sozinho: ficar preso num plano terminado seria pior que o
        # problema que o pin resolve (planos completos saem da eleicao, entao terminar um devolvia o
        # painel pro plano velho e pendente que sobrou).
        pin = read_pin(root)
        if pin == PIN_NONE:
            return None   # painel/chip/barra desligados por escolha: nem a eleicao automatica roda
        if pin:
            pinned = os.path.join(root, pin + ".md")
            try:
                got = _load(pinned, os.stat(pinned).st_mtime_ns, require_started=False)
            except OSError:
                got = None      # pin apontando pra arquivo que sumiu: cai no automatico
            if got is not None and not got.complete:
                return got

        now = time.monotonic()
        hit = _discovery_cache.get(root)
        if hit is not None and now - hit[0] < _DISCOVERY_TTL:
            path = hit[1]
        else:
            path = _discover(root)
            _discovery_cache[root] = (now, path)
        if path is None:
            return None
        try:
            mtime_ns = os.stat(path).st_mtime_ns
        except OSError:
            return None
        return _load(path, mtime_ns)
    except Exception:
        # Rede de seguranca. Os modos de falha reais sao I/O e ja sao tratados por arquivo, no
        # _discover — markdown malformado nao falha ao parsear. Excecao propagada mataria o tick
        # da lista inteira.
        _log.warning("plan_progress falhou pra cwd=%r", cwd, exc_info=True)
        return None
