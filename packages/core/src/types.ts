export type State = 'working' | 'idle' | 'awaiting_input' | 'dead';

export interface LoopHistoryEntry {
  n: number;
  ts: number;
  check_exit: number | null;
  tail: string;
}

export interface LoopState {
  goal: string;
  check_cmd: string | null;
  max_iters: number;
  require_branch: boolean;
  status: 'running' | 'paused_awaiting' | 'done_claimed' | 'done' | 'stopped' | 'exhausted' | 'failed';
  iter: number;
  history: LoopHistoryEntry[];
  started_ts: number;
  ended_ts: number | null;
  ended_reason: string | null;
}

// Qual Adapter dirige a sessao (app.adapters.get_adapter no backend). "claude" e o default;
// "codex" identifica as criadas via registry.create_codex; "pi" as detectadas pelo processo do
// pane (registry.provider_of_pane); "kimi" idem ao pi (pane tmux, transcript tardio); "omp" e o
// oh-my-pi, fork do pi — mesmo transcript, mas a pergunta ao usuario vem da tool `ask`. O front usa
// isto pra esconder controles Claude-only (picker de /model, slash-commands) e pra rotular a
// sessao (lib/format.providerName).
// Tipo nomeado porque a mesma uniao viaja pela cadeia de criacao (CreateSessionSheet -> handleCreate
// das duas views -> api.createSession): quando o Pi entrou, as copias literais ficaram pra tras.
export type Provider = 'claude' | 'codex' | 'pi' | 'kimi' | 'omp';

export interface SessionInfo {
  name: string;
  cwd?: string;
  jsonl?: string | null;
  provider?: Provider;
  state: State;
  last_activity?: number | null;
  // Vínculo nome<->transcript confiável? false = claude manual sem --session-id (chute mtime) ->
  // marca "sem id" e bloqueia o chat (evita mostrar/trocar a conversa errada).
  tracked?: boolean;
  branch?: string | null;   // branch git atual do cwd (mostrada na lista de sessões)
  /** cwd é uma worktree ligada (`.git` arquivo) — marcador ⧉ ao lado da branch nas duas listas. */
  worktree?: boolean;
  // Linhas adicionadas/removidas no working tree vs HEAD (git diff --numstat, staged+unstaged;
  // untracked não conta). null = cwd sem repo ou repo sem commit nenhum -> sem badge.
  git_added?: number | null;
  git_removed?: number | null;
  // Estado vivo detalhado, pra a linha ser acionável sem abrir a sessão (feature #1):
  label?: string | null;       // working: texto do spinner
  question?: string | null;    // awaiting_input: a pergunta
  options?: string[] | null;   // awaiting_input: rótulos das opções
  // True quando "working" ha mais de CP_STALL_SECONDS sem avancar (feature #7: watchdog de travada) —
  // so tinge a linha; o backend (stall_watch.py) e quem decide o push.
  stalled?: boolean;
  // Problema DESTA sessão que o app mostra em vez de esconder. Vem como CÓDIGO do backend e é
  // traduzido aqui (regra de i18n). Hoje só 'codex_hooks_nao_aprovados': sessão Codex com turno
  // andando no rollout e nenhum marcador de estado — sem isto ela apareceria ociosa trabalhando.
  problema?: string | null;
  // Só na resposta do POST /api/sessions: avisos da reconciliação da conta (plugin ligado sem
  // instalação, /model desfeito pelo principal). Texto já pronto do backend; a Sidebar mostra em flash.
  avisos?: string[];
  // Feature #8 (rate-limit radar): banner de limite de uso detectado no pane (best-effort). limit_reset
  // = horario cru do reset ("3pm"/"15:30"), pro chip "limitado · HH:MM".
  limited?: boolean;
  limit_reset?: string | null;
  // Feature #12 (encadeamento de sessao): nome da sessao ALVO se ha um vinculo 'then' armado
  // ("quando terminar -> enviar pra"), null senao. So o alvo (indicador na lista); o texto do prompt
  // fica no backend, so exposto na hora de setar/limpar.
  then_target?: string | null;
  pair_peers?: string[] | null; // grupo de trabalho ativo: os OUTROS membros (2 sessões = lista de 1)
  pair_gid?: string | null;     // id estável do grupo (cluster da lista agrupa por ele)
  pair_task?: string | null;    // rótulo do grupo (ex: ABC-1234) pro header do cluster
  // Statusline crua da sessão (cache ~20s no backend) — o card do board/canvas parseia com
  // parseStatusLine (modelo/contexto no composer; ⚡5h/📅7d na RateStrip). Chat usa a versão ao vivo.
  status_line?: string | null;
  // Feature loop runner (Task 9+): estado de um loop autônomo por sessão
  loop_status?: LoopState['status'] | null;
  loop_iter?: number | null;    // iteração atual dentro do loop
  loop_max?: number | null;     // máximo de iterações permitidas
  // Progresso do plano do superpowers em execução nesta sessão (app/planprog.py). null = sem plano.
  plan_name?: string | null;        // nome do plano do superpowers em execução
  plan_task?: number | null;        // Task atual (ordinal, 1-based)
  plan_task_total?: number | null;
  plan_done?: number | null;        // steps marcados
  plan_total?: number | null;
  plan_complete?: boolean | null;
  plan_tasks?: [number, number][] | null;   // (done,total) por Task — alimenta a barra segmentada
  // true = o usuário escolheu "nenhum plano". Distinto de "não há plano": mantém o painel montado
  // (é lá que mora o seletor pra desfazer a escolha).
  plan_hidden?: boolean | null;
  // Motor de modelo desta sessão (nome no engines.json). null/undefined = conta Anthropic.
  engine?: string | null;
  // Conta da sessão como ID do /api/cotas ("claude:<dir>", "chave:<motor>") — a pílula de cota do
  // topo mostra o uso da conta da sessão ATIVA. null = não dá pra saber (kimi/pi sem motor).
  conta?: string | null;
}

// Detalhe do plano (GET /api/sessions/{name}/plan, Task 5): granularidade de Task/Step que o
// SessionInfo.plan_* não carrega (aquele é o resumo pra chip/barra na lista). 404 = sem plano ativo.
// `idx` é a posição 0-based do step na ordem do DOCUMENTO (não dentro da Task): é a chave que o
// POST /plan-step devolve pro backend, porque título repete entre Tasks.
export interface PlanStep { title: string; done: boolean; manual: boolean; idx: number }
export interface PlanTask { title: string; done: number; total: number; steps: PlanStep[] }
export interface PlanDetail {
  name: string; path: string;
  stem: string;              // nome do arquivo sem .md — reabre o plano pra marcar/arquivar
  task: number; task_total: number;
  done: number; total: number; complete: boolean;
  tasks: PlanTask[];
  markdown: string;
}

// Sessão marcada com o servidor de origem (visão agregada multi-servidor).
export interface AggSession extends SessionInfo {
  serverId: string;
  serverLabel: string;
  serverColor: string;
}

// Candidato de resume: um transcript <uuid>.jsonl do cwd que a sessão "sem id" poderia retomar.
export interface ResumeCandidate {
  session_id: string;
  mtime?: number | null;
  preview: string;         // 1ª msg de usuário da conversa (pra reconhecer qual é)
  in_use: boolean;         // já é o transcript de outra sessão viva -> retomar aqui roubaria
}

// Resposta do /resume: ou a sessão já religada, ou (caso ambíguo) os candidatos pra confirmar.
export type ResumeResult = SessionInfo | { ambiguous: true; candidates: ResumeCandidate[] };

export interface ChatEvent {
  // `thinking` = resumo do raciocínio. Só chega quando a sessão foi aberta com o resumo ligado
  // (settings.json["showThinkingSummaries"]); no Pi e no Kimi o texto é o raciocínio cru.
  kind: 'user_msg' | 'assistant_msg' | 'tool_use' | 'tool_result' | 'thinking';
  id: string;
  text?: string | null;
  tool_name?: string | null;
  tool_input?: Record<string, unknown> | null;
  tool_use_id?: string | null;
  result?: string | null;
  is_error?: boolean | null;
  ts?: number | null;
  // Cache de prompt (só em assistant_msg): tokens lidos do cache + janela de expiração em segundos.
  // O TTL vem medido do usage do transcript (1h ou 5min), não suposto.
  cache_read?: number | null;
  cache_ttl_s?: number | null;
  image_count?: number | null;   // imagens coladas no terminal -> busca lazy em /transcript-image
  desistiu?: boolean | null;     // só em bolhas "queued-" e "held-" (não em evento real): entrega
                                 // dada como perdida — a TUI engoliu as teclas ou o hook barrou o prompt
}

export interface StateEvent {
  session: string;
  state: State;
  label?: string | null;
  question?: string | null;
  options?: string[] | null;
  status_line?: string | null; // raw bottom chrome from the pane, shown as-is
  overlay?: boolean;            // overlay so-TUI aberto (/status, /config, /help, picker) -> espelho do pane
  login?: boolean;             // sessao na tela de welcome/login do Claude Code -> avisa + abre o espelho
  // Feature #8 (rate-limit radar): banner de limite de uso no pane (best-effort). limit_reset = horario
  // cru do reset ("3pm"/"15:30"), ou null.
  limited?: boolean;
  limit_reset?: string | null;
  // Loop runner: campos no SSE DA SESSÃO (chip do header do chat) — o Chat NÃO usa o
  // sessionsStore pra isso (reter o store no mobile abria N streams de lista e derrubava
  // a conexão do celular; regressão real revertida).
  loop_status?: LoopState['status'] | null;
  loop_iter?: number | null;
  loop_max?: number | null;
}

// Evento SSE `preview` (app/preview.py): texto em voo, full-replace. md = veio do sidecar do
// agente (markdown cru); full = bloco inteiro (não raspagem parcial do pane).
export interface PreviewEvent { text?: string; md?: boolean; full?: boolean }

// Faixa de estatísticas da sessão (evento SSE `stats`, app/stats.py). Todos os campos
// além de turns/steps/in_tok/out_tok são opcionais — o front só desenha o que veio.
// Tempos/velocidade são aproximados por construção (o front prefixa "~").
export interface StatsEvent {
  turns: number;
  steps: number;
  in_tok: number;
  out_tok: number;
  llm_ms?: number;
  tool_ms?: number;
  tok_s?: number;
  cache_pct?: number;
  ttft_ms?: number;
}

export interface CommandInfo {
  name: string;
  display: string;                 // forma exibida, ex: "/clear"
  description?: string | null;
  argumentHint?: string | null;    // dica de argumento, ex: "<ambiente>"
  source: 'builtin' | 'skill' | 'plugin';
  destructive?: boolean;           // exige confirmação antes de enviar
}

// ── Workflows (painel estilo /workflows do terminal) ────────────────────────
export interface WorkflowSummary {
  runId: string;
  name: string;
  status: string; // completed | killed | running
  agentCount: number;
  phaseCount: number;
  totalTokens: number;
  durationMs: number;
  startTime: number;
  running: boolean;
}

export interface WorkflowAgent {
  agentId: string | null;
  label: string | null;
  phaseTitle: string | null;
  state: string | null; // done | error | progress
  model: string | null;
  tokens: number;
  durationMs: number;
  toolCalls: number;
  lastToolName: string | null;
  lastToolSummary: string | null;
  resultPreview: string | null;
}

// Subagente solto (tool Agent), lido do transcript dele em <session-dir>/subagents/.
export interface SubagentRun {
  agentId: string;
  agentType: string | null;
  prompt: string | null;
  startedAt: string;
  updatedAt: string;
  mtime: number;
  toolCalls: number;
  tools: { name: string; count: number }[];
  recent: { name: string; target: string }[];
  lastText: string;
  // So o Kimi manda: o wire dele registra o fim do turno do proprio subagente. Ausente (Claude) =
  // "nao da pra saber por aqui" — quem decide la e o tool_result no transcript do pai.
  finished?: boolean;
  // O subagente existe mas o transcript dele nao deu pra ler agora (permissao, disco). Os outros
  // campos vem zerados: `toolCalls: 0` aqui NAO quer dizer "nao chamou nada", quer dizer "nao sei".
  ilegivel?: boolean;
  // Transcript do subagente nos MESMOS ChatEvent do chat (so com ?events=N).
  events?: ChatEvent[];
}

export interface WorkflowAgentDetail {
  agentId: string;
  label: string;
  phaseTitle: string | null;
  state: string | null;
  model: string | null;
  tokens: number;
  durationMs: number;
  toolCalls: number;
  prompt: string | null;
  result: string | null;
  tools: { name: string; count: number }[];
  lastToolName: string | null;
  lastToolTarget: string | null;
}

export interface WorkflowDetail {
  runId: string;
  name: string;
  status: string;
  totalTokens: number;
  durationMs: number;
  summary: string | null;
  phases: { title: string | null; detail: string | null }[];
  agents: WorkflowAgent[];
}

// ── Config dirs do Claude (~/.claude, alternativas) ────────────────────────
export interface ConfigDirInfo {
  path: string;
  label: string;
  active: boolean;
}

// ── Scanner de pastas ───────────────────────────────────────────────────────
export interface FsRoot {
  name: string;   // basename da raiz (vira o rótulo do chip)
  path: string;   // caminho absoluto da raiz liberada
}

export interface FsEntry {
  name: string;
  path: string;
  is_git: boolean;
  has_claude_md: boolean;
  mtime?: number | null;
}

// Estado de falha da varredura, mapeado pra uma mensagem visível na UI.
export type FsScanError =
  | 'permission_denied'
  | 'unreadable'
  | 'root_not_allowed'
  | 'invalid_path'
  | 'not_found'
  | 'unknown';

export interface FsScanResult {
  entries: FsEntry[];
  error?: FsScanError | null;
}

// ── AskUserQuestion (stepper nativo multi-pergunta) ─────────────────────────
export interface AskOption { label: string; description: string; preview?: string }
export interface AskQuestionItem { header: string; question: string; multiSelect: boolean; options: AskOption[] }
export interface AskQuestionPayload { questions: AskQuestionItem[] }
export type AnswerItem =
  | { kind: 'option'; indices: number[]; multi: boolean; labels: string[] }
  | { kind: 'text'; value: string; type_index: number; labels: string[] }
  | { kind: 'chat'; chat_index: number };

// ── Custos (visão agregada de uso/gasto por conta) ──────────────────────────
export interface CostBucket {
  key: string;
  sessions: number;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  cost: number;
}

// ── Custos v2 (dashboard multi-agente: Claude Code + Codex + Pi) ────────────
// Um corte qualquer (dia, provedor, fonte, projeto, modelo). `key` é o valor da dimensão.
// O custo vem quebrado por tipo DENTRO do corte: é o que faz a barra do projeto mostrar a FORMA
// do gasto (projeto de output != projeto de cache) sem precisar clicar.
export interface DimBucket {
  key: string;
  // Rótulo legível quando a `key` não serve pra ler — hoje só a conta Anthropic, cuja chave é
  // 'anthropic:<uuid>' (identidade que não colide na malha) mas cujo nome é o e-mail. A tela
  // exibe `label ?? key`; ausente é o caso normal, em que a chave já é o nome.
  label?: string | null;
  sessions: number;
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  cost: number;
  cost_input: number;
  cost_output: number;
  cost_cache_write: number;
  cost_cache_read: number;
}

// Uma linha por combinação que REALMENTE aconteceu (dia × provedor × fonte × projeto × modelo ×
// subagente). É o que os `by_*` perdem ao somar: depois de somado não dá pra separar "quanto
// daquele projeto foi de tal fonte". Esparso na prática — o número de combinações que existem é
// uma fração ínfima do produto das dimensões.
export interface ComboRow {
  dia: string;
  provider: string;
  source: string;
  project: string;
  model: string;
  subagente: boolean;
  sessions: number;
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  cost: number;
  cost_input: number;
  cost_output: number;
  cost_cache_write: number;
  cost_cache_read: number;
}

// A MESMA linha, depois de carimbada com a máquina de onde veio. O carimbo é do CLIENTE: nenhum
// backend manda este campo — `mergeReports` o escreve na hora de juntar as máquinas, porque
// depois de somado não dá pra separar "quanto disso foi da VPS". Mantido FORA do `ComboRow` de
// propósito: aquele é o formato do fio, e pôr nele um campo que servidor nenhum manda apagaria a
// fronteira que o resto deste módulo se esforça pra manter.
export interface ComboLocal extends ComboRow {
  servidor: string;
}

export interface KindBucket {
  kind: string; // "input" | "output" | "cache_write" | "cache_read"
  tokens: number;
  cost: number;
}

export interface RateInfo {
  model: string;
  provider: string;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  origin: string; // "override" | "models.dev" | "snapshot"
  cache_estimado?: boolean;
}

// Eco dos filtros que o servidor REALMENTE aplicou. FastAPI ignora query param desconhecido: um
// backend antigo recebe ?period=7d e devolve TUDO. Sem este eco o front somaria "7 dias da
// máquina A" com "sempre da máquina B" e mostraria como um período só.
export interface Applied {
  period: string;
}

// Formato COMPLETO — o que mergeReports produz e a tela consome. A tolerância a servidor antigo
// mora na ENTRADA (ServerResult usa Partial<CostReport>), não aqui: deixar tudo opcional nos dois
// lados obrigaria a tela inteira a `?? 0` num dado que o merge garante existir.
export interface CostReport {
  totals: DimBucket;
  by_day: DimBucket[];
  by_provider: DimBucket[];
  by_source: DimBucket[];
  by_project: DimBucket[];
  by_model: DimBucket[];
  by_kind: KindBucket[];
  rates: RateInfo[];
  sem_tarifa: string[];
  // Quanto custaria se NENHUM token fosse cache (cache W e R a preço de input cheio).
  custo_sem_cache: number;
  // Tokens em "equivalente-input": cada tipo pesado pela PRÓPRIA tarifa. Vem do servidor porque
  // depende da tarifa de cada modelo, que o front não tem.
  equivalente_cobrado: number;
  // Totais da janela imediatamente ANTERIOR, do mesmo tamanho — a régua do "subiu ou desceu".
  // null quando não dá pra comparar (period=all, ou janela anterior com pouco registro).
  anterior: DimBucket | null;
  // Detalhamento cruzado. OPCIONAL porque servidor da malha em versão antiga não manda: sem ele a
  // tela cai no recorte de uma dimensão só, a partir dos `by_*`.
  combos?: ComboRow[];
  applied: Applied;
  usd_brl: number | null;
}

export interface Runner {
  label: string;
  command: string;
  source: 'npm' | 'make' | 'stack';
  is_dev_guess: boolean;
}

export interface RunInfo {
  command: string;
  since?: number | null;
}

export interface RunnersResponse {
  detected: Runner[];
  remembered: string | null;
  running: RunInfo | null;
}

// Janela de rate-limit normalizada (GET /limits, Task B) — so Codex; ver docs/codex-app-server-contract.md.
export interface RateLimitWindow {
  usedPercent: number | null;
  windowMins: number | null;
  resetsAt: number | null; // epoch em SEGUNDOS
}

export interface SessionLimits {
  primary: RateLimitWindow | null;
  secondary: RateLimitWindow | null;
  planType: string | null;
}

// Modelo + reasoning effort do Codex (GET /models, Task C) — normalizado no backend a partir de
// `model/list` (hidden ja filtrado). effort e string livre (o que vier de supportedReasoningEfforts).
export interface CodexModelEffort {
  value: string;
  description: string | null;
}

export interface CodexModel {
  model: string;
  displayName: string | null;
  description: string | null;
  efforts: CodexModelEffort[];
  defaultEffort: string | null;
}

export interface CodexModelChoice {
  model: string | null;
  effort: string | null;
}

export interface CodexModelsResponse {
  models: CodexModel[];
  current: CodexModelChoice;
}

// Modelo + nivel de raciocinio de uma sessao Pi (GET /pi/models). Vem do sidecar que a extensao
// hangar-state.ts publica perguntando pro proprio Pi — nao ha picker raspado aqui (o /model do Pi e
// uma lista com busca de ~300 modelos). `levels` sao os niveis que o modelo ATUAL aceita: variam
// por modelo (medido: glm-5.2 -> off/low/medium/high/xhigh; k3 -> low/high/max).
export interface PiModel {
  provider: string;
  id: string;
  name: string;
  reasoning: boolean;
  // Vêm do `pi --list-models` (fonte da lista). O sidecar não publica os dois, então são opcionais:
  // etiqueta que a fonte não informar simplesmente não aparece.
  context?: string;
  images?: boolean;
}

export interface PiModelsResponse {
  models: PiModel[];
  current: { provider: string; id: string; name?: string } | null;
  thinking: string | null;
  levels: string[];
}

// Um anexo já enviado pra sessão (GET /api/sessions/{name}/uploads) — a galeria lista a pasta
// daquela sessão no cofre. expires_in_days vem do backend (só ele conhece o prazo de retenção);
// null = retenção desligada e PODE ser <= 0: o prune só roda no próximo upload, então um anexo
// vencido continua aparecendo até lá.
export interface UploadFile {
  filename: string;
  size: number;
  mtime: number;            // epoch em SEGUNDOS
  expires_in_days: number | null;
}

// Arvore de arquivos do repo da sessao (GET /api/sessions/{name}/files/list).
// `changed` e a 1a letra do XY do porcelain (git_ops.changed_files, com strip): o backend
// devolve M/A/D/R/C/U/T/? de verdade — rename, copy, merge e troca de tipo tambem aparecem.
export interface TreeEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  changed: 'M' | 'A' | 'D' | 'R' | 'C' | 'U' | 'T' | '?' | null;
  add: number;
  del: number;
}

export interface TreeListing { entries: TreeEntry[]; truncated: boolean }

// Conteudo de UM arquivo (GET /api/sessions/{name}/files/read). `truncated` no teto de 512 KB.
// `digest` é a impressão do que foi lido; volta pro backend ao salvar, que recusa a gravação se
// o arquivo mudou no disco no meio (o agente da sessão edita os mesmos arquivos). É `null` numa
// leitura truncada — e sem digest não há gravação.
export interface FileContent { path: string; text: string; size: number; truncated: boolean; digest: string | null }

// Um achado da busca (GET /api/sessions/{name}/files/search). No modo `names`, line e text vem null.
export interface FileSearchHit { path: string; line: number | null; text: string | null }

export interface SearchResult { hits: FileSearchHit[]; truncated: boolean; mode: 'names' | 'contents' }

// Diff de UM arquivo (POST /api/sessions/{name}/git/path-diff). `escopo_usado` pode divergir do
// pedido: `branch` cai pra `nao_commitado` quando a base nao da pra achar (base/motivo dizem o porque).
// `motivo` e um CODIGO (chave `arq_*`) que o front traduz via errosApi — nunca frase; codigo
// desconhecido simplesmente nao desenha (FileViewer).
export interface PathDiff {
  path: string;
  diff: string;
  truncated: boolean;
  escopo_pedido: 'branch' | 'nao_commitado';
  escopo_usado: 'branch' | 'nao_commitado';
  base: string | null;
  motivo: string | null;
  // Texto do arquivo na base — o lado esquerdo do diff, pro visor desenhá-lo dentro do editor.
  // `""` = arquivo novo (tudo é adição); `null` = não há versão na base ou o arquivo é grande demais.
  original: string | null;
}

// ── Orquestração (GET /api/orq, /api/orq/{id}) ────────────────────────
// Espelha as dataclasses de backend/app/orq.py. `eventos`/`eventos_execucao` só vêm no DETALHE —
// a lista os corta pra não trafegar a linha do tempo inteira de toda execução.
export interface OrqEvento {
  ts: string;
  tipo: 'execucao_inicio' | 'task_inicio' | 'entrega' | 'veredito' | 'sessao_trocada' | 'execucao_fim';
  task?: number;
  rodada?: number;
  resultado?: string;
  sessao?: string;
  commit?: string;
  motivo?: string;
  de?: string;
  para?: string;
  titulo?: string;
  executor?: string;
  par?: string;
  ts_aproximado?: boolean;
}

export interface OrqTask {
  task: number;
  titulo: string;
  executor: string;
  par: string;
  rodadas: number;      // 0 = desconhecido (o arquivo omitiu a rodada), nunca "de primeira"
  resultado: string | null;
  inicio: string | null;
  fim: string | null;
  eventos?: OrqEvento[];
}

export interface OrqExecucao {
  id: string;
  plano: string;
  branch: string;
  gid: string;
  inicio: string | null;
  fim: string | null;          // null = execução viva
  resultado: string | null;
  tasks: OrqTask[];
  eventos_execucao?: OrqEvento[];
  voltas: number;
  aprovadas_primeira: number;
  reconstruida: boolean;
}

export interface OrqFicha {
  par: string;
  aceitas: number;
  nao_aceitas: number;
  aprovadas_primeira: number;
  rodadas_media: number;
}

export interface OrqLista {
  execucoes: OrqExecucao[];
  fichas: OrqFicha[];
}

// ── Botão Atualizar ────────────────────────────────────────────────────────────────────────────

export interface AtualizacaoMudanca {
  sha: string;
  titulo: string;
}

export interface AtualizacaoPasso {
  id: string;
  titulo: string;
  texto: string;
}

/** O que o motor está fazendo agora. Vem do arquivo de estado, que sobrevive ao restart. */
export interface AtualizacaoEstado {
  fase?: 'rodando' | 'pronto';
  etapa?: string;
  passo?: number;
  total?: number;
  texto?: string;
  ok?: boolean | null;
  erro?: string;
  resgate?: string | null;
  /** O código anterior foi restaurado no disco. */
  voltou?: boolean;
  /** O servidor respondeu depois disso. Separado de `voltou`: reverter e não subir é um terceiro caso. */
  no_ar?: boolean;
  reiniciar_manual?: boolean;
  /** O último reinício pedido pela tela falhou. Escrito pelo processo destacado, único canal dele. */
  reinicio_erro?: string | null;
  /** Arquivos de `docs/atualizacoes/` que o app ignorou por estarem malformados. */
  passos_invalidos?: string[];
  /** Comandos e saída, pra tela poder mostrar o que está rodando agora. */
  log?: string[];
  /** Quando a etapa atual começou — a tela conta daqui pra mostrar que não travou. */
  etapa_inicio?: string;
  /** Terminou bem, mas algo ficou pra trás (ex: dependências da janela nativa). */
  avisos?: string[];
  /** O pull tocou `shell/`: a janela nativa só pega o código novo ao fechar e abrir o app. */
  shell_mudou?: boolean;
  commit_de?: string;
  commit_para?: string;
  ts?: string;
}

export interface AtualizacaoPreVoo {
  pode: boolean;
  faltando: string[];
  branch?: string;
  sujo?: number;
  ahead?: number;
  behind?: number;
  divergiu?: boolean;
  topologia?: 'systemd' | 'windows' | 'manual';
  /** Checkout fora de main/master: atualizar arrastaria a branch de trabalho, então o backend recusa. */
  branch_de_trabalho?: boolean;
}

export interface Atualizacao {
  /** `repo` é o disco; `backend` é o processo vivo. Divergem durante a atualização — é o ponto. */
  versoes: { repo: string; backend: string };
  atualizacao_disponivel: boolean;
  mudancas: AtualizacaoMudanca[];
  passos: AtualizacaoPasso[];
  pre_voo: AtualizacaoPreVoo;
  estado: AtualizacaoEstado;
}
