import json
import re
from pathlib import Path
from typing import Any, Literal, Optional
from pydantic import BaseModel, Field, model_validator

ChatKind = Literal["user_msg", "assistant_msg", "tool_use", "tool_result", "thinking"]
State = Literal["working", "idle", "awaiting_input", "dead"]

# Surrogate SOLTO (sem par) num str vindo de json.loads. O json aceita "\ud83d" sozinho e o Python
# monta o str, mas serializar de volta pra JSON estoura
# `UnicodeEncodeError: surrogates not allowed` -> 500 no /history inteiro e o pump do SSE morre.
# Acontece de verdade: o Pi trunca texto longo por UNIDADE UTF-16 e parte um emoji ao meio ao
# gravar o proprio JSONL. json.loads ja junta um par bem-formado num codepoint unico, entao todo
# surrogate que sobra aqui e lixo — troca por U+FFFD (o replacement char), que MOSTRA que faltou
# algo em vez de sumir e deslocar o texto.
_SURROGATE_RE = re.compile(r"[\ud800-\udfff]")


def scrub_surrogates(v: Any) -> Any:
    """Troca surrogate solto por U+FFFD, recursivo em dict/list. STRING bem-formada sai IDENTICA
    (o mesmo objeto): o search e um scan em C e so paga a substituicao quando ha o que consertar.
    dict/list, ao contrario, sao SEMPRE remontados — o conteudo sai igual, a identidade nao. E de
    proposito: a alternativa (comparar item a item pra talvez devolver o original) percorre a mesma
    estrutura e ainda alocaria a copia pra comparar. Ninguem depende da identidade do container."""
    if isinstance(v, str):
        return _SURROGATE_RE.sub("�", v) if _SURROGATE_RE.search(v) else v
    if isinstance(v, dict):
        return {scrub_surrogates(k): scrub_surrogates(x) for k, x in v.items()}
    if isinstance(v, list):
        return [scrub_surrogates(x) for x in v]
    return v


def dumps_safe(obj: Any) -> str:
    """json.dumps de algo que veio do CLIENTE, ja sem surrogate solto — o unico jeito seguro de
    serializar texto digitado pelo usuario pra um arquivo. json.dumps ACEITA "\\ud83d" sozinho e
    devolve um str feliz; quem estoura e o `.encode("utf-8")` logo depois (write_text) com
    `UnicodeEncodeError: surrogates not allowed`. Meio emoji e trivial de produzir: o browser fatia
    string por UNIDADE UTF-16, entao um corte no meio do par ja chega assim no POST. Sem isto, o
    write do sidecar (fila, chain, loop, pair) virava 500 e a mensagem do usuario sumia."""
    return json.dumps(scrub_surrogates(obj), ensure_ascii=False)


# rollout-<data>T<hora>-<uuid do Codex>. Ancorado no fim pra um arquivo que so PARECE rollout
# (sem o id) continuar caindo no stem, como sempre foi.
_ROLLOUT_ID_RE = re.compile(
    r"^rollout-.*-([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$")


def session_key(jsonl_path: str) -> str:
    """Chave de ESTADO/sidecar de um transcript (marcador .hangar-state, id de SSE).

    No Claude/Pi e o stem do arquivo (== session-id). No Kimi o transcript se chama wire.jsonl em
    TODA sessao (sessions/<wd>/<session_id>/agents/main/wire.jsonl), entao o stem seria "wire" pra
    todo mundo — a chave e o nome do sessionDir, que e o session_id que o hook grava. No Codex o
    rollout se chama rollout-<data>T<hora>-<id>.jsonl, e o <id> do fim e o mesmo `session_id` que o
    hook entrega no stdin (conferido contra o `session_meta` da primeira linha dos rollouts reais):
    o stem inteiro nunca casaria com o marcador, e a sessao ficaria eternamente ociosa. Qualquer
    outro layout cai no stem (comportamento de sempre).
    """
    p = Path(jsonl_path)
    if p.name == "wire.jsonl" and p.parent.parent.name == "agents":
        return p.parent.parent.parent.name
    m = _ROLLOUT_ID_RE.match(p.stem)
    if m:
        return m.group(1)
    return p.stem


class SessionInfo(BaseModel):
    name: str
    cwd: Optional[str] = None
    jsonl: Optional[str] = None
    # Qual Adapter dirige esta sessao (app.adapters.get_adapter). "claude" cobre TODA sessao de hoje
    # (o unico provider registrado); futuros providers (ex: "codex") setam no create().
    provider: str = "claude"
    # Motor de modelo desta sessao (nome no engines.json). None = conta Anthropic. Lido do
    # /proc/<pid>/environ (CP_ENGINE) — ver registry._engine_of.
    engine: Optional[str] = None
    # Conta da sessão como ID do /api/cotas ("claude:<config_dir>", "chave:<motor>",
    # "kimi:<provider do default_model>") — a pílula de cota do topo mostra o uso da conta da
    # sessão ATIVA a partir daqui. None quando não dá pra saber (pi, ou kimi sem provider com
    # chave): a pílula cai no pior-geral (smart).
    conta: Optional[str] = None
    state: State = "idle"
    last_activity: Optional[float] = None
    # Vinculo nome<->transcript e confiavel? True = resolvido por --session-id/fd/cache (determinismo).
    # False = chute newest-by-mtime (claude manual sem --session-id) -> UI marca "sem id" e desliga chat.
    tracked: bool = True
    branch: Optional[str] = None   # branch git atual do cwd (lida de .git/HEAD) — mostra na lista
    # True quando o cwd e uma worktree ligada (`.git` arquivo apontando pro repo principal) — a mesma
    # branch em worktrees diferentes tem arquivos diferentes, entao a lista marca qual e qual.
    worktree: bool = False
    # Estado de git do cwd, decorado em list_with_state (git_summary, cacheado). dirty = arquivos
    # não-commitados; ahead = commits não-pushados (None sem upstream real); behind idem. Non-repo
    # -> tudo None (sem badge no painel).
    git_dirty: Optional[int] = None
    git_ahead: Optional[int] = None
    git_behind: Optional[int] = None
    # Adicoes/delecoes do working tree vs HEAD (staged + unstaged), decorados junto (git_diffstat,
    # cacheado) — o "+N -M" do card. Untracked NAO conta (ver git_ops.git_diffstat). Non-repo ou
    # repo sem commits -> None (sem badge).
    git_added: Optional[int] = None
    git_removed: Optional[int] = None
    # Só na resposta do POST /api/sessions: avisos da reconciliação da conta (plugin ligado sem
    # instalação, /model desfeito pelo principal). Antes iam só pro log do backend e quem abria
    # sessão pelo app nunca via.
    avisos: list[str] = []
    # Estado vivo detalhado, pra a linha da lista ser acionável sem abrir a sessão (feature #1):
    label: Optional[str] = None          # working: texto do spinner ("Elucidating…")
    question: Optional[str] = None       # awaiting_input: a pergunta
    options: Optional[list[str]] = None  # awaiting_input: rótulos das opções
    # True quando "working" ha mais de CP_STALL_SECONDS sem avancar (last_activity parado) — feature #7:
    # loop infinito de ferramenta / subprocesso esperando stdin nunca vira awaiting/finished/dead sozinho.
    # Derivado em list_with_state(); so tinta a linha, o watchdog (stall_watch.py) e quem pinga 1x.
    stalled: bool = False
    # Problema DESTA sessão que o app tem que mostrar em vez de esconder: CÓDIGO, nunca texto — a
    # tradução é do front (regra de i18n do CLAUDE.md). Hoje só "codex_hooks_nao_aprovados": sessão
    # Codex com turno andando no rollout e marcador de estado nenhum. Sem isto ela apareceria
    # eternamente "ociosa" enquanto trabalha, que é o único modo de falha do estado por hook.
    problema: Optional[str] = None
    # Feature #8 (rate-limit radar): banner de limite de uso detectado no pane (best-effort, ver
    # app.state.rate_limit_reset). limit_reset = horario cru ("3pm"/"15:30") pro chip "limitado · HH:MM".
    # Derivado em list_with_state(); o push (1x, dedupe) e o auto-resume opt-in moram no stall_watch.py.
    limited: bool = False
    limit_reset: Optional[str] = None
    # Feature #12 (encadeamento de sessao): nome da sessao ALVO se esta sessao tem um vinculo 'then'
    # armado ("quando terminar -> enviar pra"), None senao. So o alvo (pro indicador na lista); o texto
    # do prompt fica no sidecar (app.chain.ThenLink), lido so na hora de disparar.
    then_target: Optional[str] = None
    # Statusline crua da sessao (mesma do StateEvent), pro card do board/canvas mostrar modelo/
    # contexto/rate sem SSE por sessao. Vem de um cache com TTL em list_with_state (cadencia ~20s,
    # max 2 capturas de pane por chamada) — pode atrasar; o Chat continua com a versao ao vivo.
    status_line: Optional[str] = None
    # Pareamento ativo (feature "trabalhando juntas"): os OUTROS membros do grupo, ou None.
    # Grupo de 2 = lista de 1 (o antigo 1:1 é caso particular). Badge/chip na UI.
    pair_peers: Optional[list[str]] = None
    pair_gid: Optional[str] = None   # id estável do grupo — cluster da lista agrupa por ele
    pair_task: Optional[str] = None  # rótulo do grupo (ex: ABC-1234) pro header do cluster
    # Loop runner (harness bloco A): estado do loop autonomo desta sessao, decorado em list_with_state
    # (app.loop.LoopLink). Sem loop -> tudo None (sem badge). Entram no sig do SSE de lista (sse.py)
    # pra o badge nao congelar quando so o loop muda com a sessao parada em idle.
    loop_status: Optional[str] = None
    loop_iter: Optional[int] = None
    loop_max: Optional[int] = None
    # Progresso do plano do superpowers que esta sessao esta executando (app.planprog), decorado em
    # list_with_state DENTRO do to_thread do git. Sem plano -> tudo None (sem barra nem chip).
    # plan_task e ORDINAL (existe "### Task 0" nos planos), nao o N do heading.
    plan_name: Optional[str] = None
    plan_task: Optional[int] = None
    plan_task_total: Optional[int] = None
    plan_done: Optional[int] = None
    plan_total: Optional[int] = None
    plan_complete: Optional[bool] = None
    # (done, total) por Task — a barra segmentada precisa disto. Derivar segmento de
    # plan_task/plan_task_total mentiria toda vez que uma Task anterior ficasse com step pendente
    # (acontece sempre que se pula um step de verificacao manual). Sao 3-8 pares.
    plan_tasks: Optional[list[tuple[int, int]]] = None
    # O usuario escolheu "nenhum plano" (pin sentinela). Sem plano E sem isto sao estados
    # diferentes: o seletor que desfaz a escolha mora no painel do plano, que so e montado quando
    # ha plan_name — este campo e o que o mantem na tela pra dar o caminho de volta.
    plan_hidden: Optional[bool] = None


class ChatEvent(BaseModel):
    kind: ChatKind
    id: str
    text: Optional[str] = None
    tool_name: Optional[str] = None
    tool_input: Optional[dict] = None
    tool_use_id: Optional[str] = None
    result: Optional[str] = None
    is_error: Optional[bool] = None
    ts: Optional[float] = None
    # Cache de prompt (só em assistant_msg): quantos tokens o turno LEU do cache e qual a janela
    # de expiração em segundos. O TTL não é chute — o usage do transcript separa
    # `ephemeral_1h_input_tokens` de `ephemeral_5m_input_tokens`, então dá pra dizer quanto tempo
    # ainda vale em vez de supor. Usar a cache renova o prazo, então a conta corre do ÚLTIMO turno.
    cache_read: Optional[int] = None
    cache_ttl_s: Optional[int] = None
    # Só em bolha da fila ("queued-"): a entrega foi dada como PERDIDA (a TUI engoliu as teclas e o
    # texto nunca apareceu no transcript). Precisa chegar ao front: sem este campo a bolha desistida
    # renderiza IGUAL a uma aceita, e "some sem aviso" vira "parece que foi" — que é pior, porque o
    # usuário acha que mandou. `confirmed` é o oposto e não vem: ali a bolha real já cobre.
    desistiu: Optional[bool] = None
    # Nº de imagens base64 anexadas a uma msg do user via TERMINAL (paste na TUI do Claude). O front
    # busca cada uma sob demanda em /transcript-image/{id}/{idx} (lazy; base64 não vai no payload).
    image_count: Optional[int] = None
    # Offset em BYTES logo após a linha do transcript que gerou este evento. Vira o `id:` do SSE ->
    # numa reconexão o browser devolve o último via Last-Event-ID e o tail retoma EXATAMENTE dali,
    # em vez de reenviar as últimas 200 linhas e torcer pra cobrir o buraco. Interno (o front não
    # lê este campo): exclude=True mantém o payload igual ao de hoje.
    offset: Optional[int] = Field(default=None, exclude=True)

    # Rede de seguranca na FRONTEIRA do contrato com o front: qualquer provider (Claude, Codex, Pi
    # ou o proximo) que grave um surrogate solto no transcript passaria direto pro
    # model_dump_json/JSONResponse e derrubaria o endpoint e o stream da sessao inteira — um emoji
    # cortado ao meio em UMA linha tornava a sessao impossivel de abrir. Cobre tambem o que o
    # parser nao normaliza (tool_input, id, tool_use_id). Nao muda nada em texto bem-formado.
    @model_validator(mode="before")
    @classmethod
    def _no_lone_surrogates(cls, data: Any) -> Any:
        return scrub_surrogates(data) if isinstance(data, dict) else data


class StateEvent(BaseModel):
    session: str
    state: State
    label: Optional[str] = None         # working: live status text, e.g. "Elucidating…"
    question: Optional[str] = None       # awaiting_input: the question line
    options: Optional[list[str]] = None  # awaiting_input: selectable option labels
    status_line: Optional[str] = None    # raw bottom chrome from the pane, shown as-is on the web
    # True quando um OVERLAY interativo so-TUI esta aberto (ex: /status, /config, /help, picker do
    # /model): tem rodape de navegacao ("Esc to cancel") mas NAO gera linha no .jsonl. O front abre o
    # espelho do pane (TerminalMirror) pra navegar via teclas, ja que so existe no terminal.
    overlay: bool = False
    # True quando a sessao esta na tela de welcome/login do Claude Code (escolher tema -> metodo ->
    # URL OAuth -> colar code). Pre-login NAO ha .jsonl, entao o chat fica vazio; o front usa esta
    # flag pra avisar ("precisa de login") e abrir o espelho do pane em vez de um chat morto.
    login: bool = False
    # Feature #8 (rate-limit radar): banner de limite de uso no pane (best-effort, ver
    # app.state.rate_limit_reset). limit_reset = horario cru do reset ("3pm"/"15:30"), ou None.
    limited: bool = False
    limit_reset: Optional[str] = None
    # Loop runner: estado do loop desta sessao, no MESMO evento 'state' (chip 🔁 no Chat mobile sem
    # reter o sessionsStore -> uma conexao SSE por sessao, nao N). Entram no dedupe do StateMonitor.
    loop_status: Optional[str] = None
    loop_iter: Optional[int] = None
    loop_max: Optional[int] = None


class PreviewEvent(BaseModel):
    # Preview AO VIVO (best-effort) do bloco de assistente em andamento.
    # Texto-completo (full-replace), substituído pela mensagem canônica do .jsonl quando o bloco fecha.
    session: str
    text: str
    # `md`: o texto é markdown CRU (veio do proprio agente — sidecar do Pi, deltas do app-server do
    # Codex) e a bolha deve RENDERIZAR. False = raspado do pane, ou seja ja pintado pela TUI, e
    # renderizar de novo estragaria o que ja esta formatado. Sem esta flag o `**negrito**` do Pi
    # aparecia cru na previa — o front mostra previa em texto plano desde que a unica fonte era o pane.
    md: bool = False
    # `full`: o texto e INCREMENTAL (so cresce no fim) — deltas do agente, ou a costura do pane do
    # Kimi (ver _costurar em preview.py). So faz diferenca com md=False: a previa raspada comum tem
    # teto de 10 linhas porque troca inteira e instavel; a costurada nao troca, entao o front mostra
    # sem teto, igual ao ramo md. Fontes md=True sao incrementais por construcao (full sempre True).
    full: bool = False


class CommandInfo(BaseModel):
    # Contrato JSON consumido pelo frontend: argumentHint em camelCase de proposito.
    name: str
    display: str                                   # forma exibida, ex: "/clear"
    description: Optional[str] = None
    argumentHint: Optional[str] = None             # dica de argumento, ex: "<ambiente>"
    source: Literal["builtin", "skill", "plugin"] = "builtin"
    destructive: bool = False                      # exige confirmacao na UI


class FsRoot(BaseModel):
    # Raiz liberada do scanner, virando um chip no app. name = basename do caminho.
    name: str
    path: str


class FsEntry(BaseModel):
    # Subdiretorio imediato listado pelo scanner.
    name: str
    path: str
    is_git: bool = False           # tem .git -> badge "git"
    has_claude_md: bool = False    # tem CLAUDE.md -> badge "CLAUDE.md"
    mtime: Optional[float] = None  # epoch s; o app formata o tempo relativo


class FsScanResult(BaseModel):
    # entries vazio + error preenchido = pasta valida porem ilegivel (ex: sem permissao).
    entries: list[FsEntry] = []
    error: Optional[str] = None


class AskOption(BaseModel):
    label: str
    description: str = ""
    # Mockup/código renderizado ao lado da opção (feature "preview" do AskUserQuestion do harness).
    # Sem ele o pydantic descartava o campo e o stepper não tinha como mostrar.
    preview: str = ""


class AskQuestionItem(BaseModel):
    header: str
    question: str
    multiSelect: bool = False
    options: list[AskOption]


class AskQuestion(BaseModel):
    questions: list[AskQuestionItem]


class KindBucket(BaseModel):
    kind: str            # "input" | "output" | "cache_write" | "cache_read"
    tokens: int = 0
    cost: float = 0.0


class DimBucket(BaseModel):
    """Um corte qualquer (dia, provedor, fonte, projeto, modelo). `key` é o valor da dimensão."""
    key: str
    # Rótulo legível quando a `key` não serve pra ler — hoje só a conta Anthropic, cuja chave é
    # 'anthropic:<uuid>' (identidade que não colide na malha) mas cujo nome é o e-mail. O front
    # exibe `label ?? key`; None é o caso normal, em que a chave já é o nome.
    label: Optional[str] = None
    sessions: int = 0
    input: int = 0
    output: int = 0
    cache_write: int = 0
    cache_read: int = 0
    cost: float = 0.0
    # Custo quebrado por tipo, DENTRO do corte: é o que faz a barra do projeto mostrar a FORMA
    # do gasto (projeto de output != projeto de cache) sem precisar clicar.
    cost_input: float = 0.0
    cost_output: float = 0.0
    cost_cache_write: float = 0.0
    cost_cache_read: float = 0.0


class RateInfo(BaseModel):
    model: str
    provider: str
    input: float
    output: float
    cache_read: float
    cache_write: float
    origin: str              # "override" | "models.dev" | "snapshot"
    cache_estimado: bool = False


class ComboRow(BaseModel):
    """Uma linha por combinação que REALMENTE aconteceu.

    Os quatro agrupamentos do CostReport somam ANTES de mandar, e depois de somado não dá pra
    separar "quanto daquele projeto foi de tal fonte" — como saber o gasto no mercado e o
    gasto em carne não diz quanta carne veio daquele mercado. Aqui cada linha é uma
    combinação, então qualquer recorte vira uma soma no cliente.

    É ACRÉSCIMO, não substituição: os `by_*` continuam, e cliente antigo ignora esta chave.
    Esparso na prática — medido em 01/08/2026 (número cresce toda semana, é foto, não
    constante): 472 combinações / 237 KB em 30 dias, 607 combinações / 302 KB em `period=all`.
    """
    dia: str
    provider: str
    source: str
    project: str
    model: str
    subagente: bool = False
    sessions: int = 0
    input: int = 0
    output: int = 0
    cache_write: int = 0
    cache_read: int = 0
    cost: float = 0.0
    cost_input: float = 0.0
    cost_output: float = 0.0
    cost_cache_write: float = 0.0
    cost_cache_read: float = 0.0


class Applied(BaseModel):
    """Eco dos filtros que o servidor REALMENTE aplicou.

    FastAPI ignora query param desconhecido: um backend antigo recebe ?period=7d e devolve TUDO.
    Sem este eco, o front somaria '7 dias da máquina A' com 'sempre da máquina B' e mostraria como
    um período só. Campo ausente na resposta = servidor antigo = parcial declarado, nunca somado.
    """
    period: str = "all"


class CostReport(BaseModel):
    totals: DimBucket = DimBucket(key="totals")
    by_day: list[DimBucket] = []
    by_provider: list[DimBucket] = []
    by_source: list[DimBucket] = []
    by_project: list[DimBucket] = []
    by_model: list[DimBucket] = []
    by_kind: list[KindBucket] = []
    rates: list[RateInfo] = []
    sem_tarifa: list[str] = []
    # Quanto custaria se NENHUM token fosse cache (cache W e R a preço de input cheio). É a
    # métrica de eficiência: medido, o cache responde por 85% de economia.
    custo_sem_cache: float = 0.0
    # Tokens em "equivalente-input": cada tipo pesado pela PRÓPRIA tarifa. É o terceiro dos
    # quatro números do topo — passaram 22 Bi brutos, pesaram como 3,3 Bi, deram 20 mil dólares.
    # Sai daqui e não do front porque depende da tarifa de cada modelo, que o front não tem.
    equivalente_cobrado: int = 0
    # Totais da janela imediatamente ANTERIOR, do mesmo tamanho — a régua do "subiu ou desceu".
    # None quando não dá pra comparar: `period=all` (não existe anterior) ou janela anterior com
    # menos de 1/3 dos dias com registro. Medido: o histórico começa em julho, então "30 dias
    # anteriores" tinha 3 dias de dado e a variação deu ▲574% — não era crescimento, era o vazio
    # dividindo. O front NUNCA calcula isto: o corte do dia é do servidor, no fuso do servidor.
    anterior: Optional[DimBucket] = None
    applied: Optional[Applied] = None
    usd_brl: Optional[float] = None
    combos: list[ComboRow] = []


class Runner(BaseModel):
    label: str
    command: str
    source: Literal["npm", "make", "stack"] = "npm"
    is_dev_guess: bool = False


class RunInfo(BaseModel):
    command: str
    since: Optional[int] = None
    # Pane morto (remain-on-exit): o processo saiu mas o log ficou. Distingue "falhou" de
    # "parado" — sem isto o run que morria logo apos o play sumia sem rastro.
    exited: bool = False
    exit_status: Optional[int] = None


class ProjectStatus(BaseModel):
    name: str
    cwd: str
    command: str
    port: Optional[int] = None
    # external: porta aberta SEM pane nosso — o projeto roda fora do launcher (subido na mao).
    state: Literal["stopped", "starting", "running", "failed", "external"]
    since: Optional[int] = None
    exit_status: Optional[int] = None
    tmux: str = ""  # sessao no socket dedicado do runner (pro attach de log completo)
    # Tem stop_command no config — unico jeito de parar um run externo (nao ha pane pra matar).
    has_stop_command: bool = False


class RunnersResponse(BaseModel):
    detected: list[Runner]
    remembered: Optional[str] = None
    running: Optional[RunInfo] = None


class RunBody(BaseModel):
    command: str
