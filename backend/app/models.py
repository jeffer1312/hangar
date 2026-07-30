import json
import re
from typing import Any, Literal, Optional
from pydantic import BaseModel, Field, model_validator

ChatKind = Literal["user_msg", "assistant_msg", "tool_use", "tool_result"]
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
    state: State = "idle"
    last_activity: Optional[float] = None
    # Vinculo nome<->transcript e confiavel? True = resolvido por --session-id/fd/cache (determinismo).
    # False = chute newest-by-mtime (claude manual sem --session-id) -> UI marca "sem id" e desliga chat.
    tracked: bool = True
    branch: Optional[str] = None   # branch git atual do cwd (lida de .git/HEAD) — mostra na lista
    # Estado de git do cwd, decorado em list_with_state (git_summary, cacheado). dirty = arquivos
    # não-commitados; ahead = commits não-pushados (None sem upstream real); behind idem. Non-repo
    # -> tudo None (sem badge no painel).
    git_dirty: Optional[int] = None
    git_ahead: Optional[int] = None
    git_behind: Optional[int] = None
    # Estado vivo detalhado, pra a linha da lista ser acionável sem abrir a sessão (feature #1):
    label: Optional[str] = None          # working: texto do spinner ("Elucidating…")
    question: Optional[str] = None       # awaiting_input: a pergunta
    options: Optional[list[str]] = None  # awaiting_input: rótulos das opções
    # True quando "working" ha mais de CP_STALL_SECONDS sem avancar (last_activity parado) — feature #7:
    # loop infinito de ferramenta / subprocesso esperando stdin nunca vira awaiting/finished/dead sozinho.
    # Derivado em list_with_state(); so tinta a linha, o watchdog (stall_watch.py) e quem pinga 1x.
    stalled: bool = False
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
    # Preview AO VIVO (best-effort) do bloco de assistente em andamento, lido do pane via capture.
    # Texto-completo (full-replace), substituído pela mensagem canônica do .jsonl quando o bloco fecha.
    session: str
    text: str


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


class Bucket(BaseModel):
    key: str
    sessions: int
    input: int
    output: int
    cache_read: int
    cache_write: int
    cost: float


class ModelBucket(BaseModel):
    model: str
    sessions: int
    cost: float
    # Default 0 e não obrigatórios: a UI junta relatórios de VÁRIOS servidores da malha, que nem
    # sempre estão na mesma versão. Servidor antigo responde sem estes campos — com default, a
    # linha dele entra com token zerado; sem, a resposta inteira dele viraria erro de validação e
    # o custo daquela máquina sumiria da soma.
    input: int = 0
    output: int = 0
    cache_read: int = 0
    cache_write: int = 0


class AccountCost(BaseModel):
    account_id: str
    email: Optional[str] = None
    label: str
    totals: Bucket
    today: float
    yesterday: float
    by_day: list[Bucket]
    by_week: list[Bucket]
    by_month: list[Bucket]
    by_model: list[ModelBucket]


class CostReport(BaseModel):
    accounts: list[AccountCost]
    usd_brl: Optional[float] = None  # cotação p/ exibir em R$ no front; None = indisponível


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
