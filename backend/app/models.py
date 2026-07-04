from typing import Literal, Optional
from pydantic import BaseModel

ChatKind = Literal["user_msg", "assistant_msg", "tool_use", "tool_result"]
State = Literal["working", "idle", "awaiting_input", "dead"]


class SessionInfo(BaseModel):
    name: str
    cwd: Optional[str] = None
    jsonl: Optional[str] = None
    state: State = "idle"
    last_activity: Optional[float] = None
    # Vinculo nome<->transcript e confiavel? True = resolvido por --session-id/fd/cache (determinismo).
    # False = chute newest-by-mtime (claude manual sem --session-id) -> UI marca "sem id" e desliga chat.
    tracked: bool = True
    branch: Optional[str] = None   # branch git atual do cwd (lida de .git/HEAD) — mostra na lista
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
    # Nº de imagens base64 anexadas a uma msg do user via TERMINAL (paste na TUI do Claude). O front
    # busca cada uma sob demanda em /transcript-image/{id}/{idx} (lazy; base64 não vai no payload).
    image_count: Optional[int] = None


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


class Runner(BaseModel):
    label: str
    command: str
    source: Literal["npm", "make", "stack"] = "npm"
    is_dev_guess: bool = False


class RunInfo(BaseModel):
    command: str
    since: Optional[int] = None


class RunnersResponse(BaseModel):
    detected: list[Runner]
    remembered: Optional[str] = None
    running: Optional[RunInfo] = None


class RunBody(BaseModel):
    command: str
