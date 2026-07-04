import { getBaseUrl, getToken, dropActiveServer } from './auth';
import type { Server } from './auth';
import type {
  SessionInfo,
  ChatEvent,
  CommandInfo,
  ConfigDirInfo,
  FsRoot,
  FsScanResult,
  FsScanError,
  WorkflowSummary,
  WorkflowDetail,
  WorkflowAgentDetail,
  AnswerItem,
  CostReport,
  ResumeResult,
  RunnersResponse,
  RunInfo,
} from './types';

// URL da idx-ésima imagem (colada no terminal) de uma msg do transcript. `?token` porque <img> não
// manda header Authorization e cross-origin (multi-PC) não leva cookie — o backend aceita ?token.
export function transcriptImageUrl(name: string, id: string, idx: number): string {
  const t = getToken() ?? '';
  return `${getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/transcript-image/${encodeURIComponent(id)}/${idx}?token=${encodeURIComponent(t)}`;
}

// URL pra servir um arquivo CITADO na conversa (video/html/pdf/img por caminho). `?token` p/ <img>/
// <video>/<iframe> (sem header). O backend so serve se o path estiver no transcript da sessao.
export function fileUrl(name: string, path: string): string {
  const t = getToken() ?? '';
  return `${getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/file?path=${encodeURIComponent(path)}&token=${encodeURIComponent(t)}`;
}

// URL de uma imagem ENVIADA do phone (upload), servida por <cwd>/.claude-pocket-uploads/<basename>.
// `?token` igual as de cima: <img> nao manda header Authorization e cross-origin nao leva cookie.
export function uploadUrl(name: string, filename: string): string {
  const t = getToken() ?? '';
  return `${getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/uploads/${encodeURIComponent(filename)}?token=${encodeURIComponent(t)}`;
}

function authHeaders(): HeadersInit {
  const token = getToken();
  if (!token) return {};
  return { Authorization: `Bearer ${token}` };
}

// Trata a resposta compartilhada por apiFetch e uploadFile. Self-heal de token invalido/rotacionado:
// isAuthenticated() so checa se EXISTE token, nao se vale. Num 401 COM token salvo, limpamos a
// credencial e recarregamos -> cai no Login pra re-parear (QR). O guard getToken() evita loop quando
// ja estamos deslogados (Login nao chama a API). Qualquer outro !ok vira erro com o corpo.
async function ensureOk(res: Response): Promise<void> {
  if (res.status === 401 && getToken()) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw new Error('401: sessão expirada — faça login novamente');
  }
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    // FastAPI devolve {"detail": "..."} -> mostra a mensagem limpa, nao o JSON cru.
    let detail = text;
    try {
      const j = JSON.parse(text);
      if (j && typeof j.detail === 'string') detail = j.detail;
    } catch { /* corpo nao-JSON: mantem o texto cru */ }
    throw new Error(`${res.status}: ${detail}`);
  }
}

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const base = getBaseUrl();
  const url = `${base}${path}`;
  const res = await fetch(url, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      ...authHeaders(),
      ...(init?.headers ?? {}),
    },
  });
  await ensureOk(res);
  return res.json() as Promise<T>;
}

export function getSessions(): Promise<SessionInfo[]> {
  return apiFetch<SessionInfo[]>('/api/sessions');
}

// Lista sessões de UM servidor específico (baseUrl+token explícitos), sem mexer no ativo. A visão
// agregada chama um por um e renderiza cada resposta assim que chega (sem esperar os outros), então
// um servidor lento/offline não segura os demais. Timeout de 4s: servidor morto falha rápido (< o
// intervalo de poll de 5s) em vez de pendurar no timeout default do browser.
export async function fetchSessionsForServer(s: Server): Promise<SessionInfo[]> {
  const res = await fetch(`${s.baseUrl}/api/sessions`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(4000),
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<SessionInfo[]>;
}

// Custo de UM servidor (baseUrl+token explicitos), sem mexer no ativo. Igual fetchSessionsForServer:
// a visao agregada chama todos em paralelo; um servidor lento/offline falha rapido (timeout 4s) e e
// pulado, sem segurar os demais.
export async function fetchCostsForServer(s: Server): Promise<CostReport> {
  const res = await fetch(`${s.baseUrl}/api/costs`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(4000),
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<CostReport>;
}

export function listClaudeConfigs(): Promise<ConfigDirInfo[]> {
  return apiFetch<ConfigDirInfo[]>('/api/claude-configs');
}

export function createSession(name: string, cwd?: string, configDir?: string | null): Promise<SessionInfo> {
  return apiFetch<SessionInfo>('/api/sessions', {
    method: 'POST',
    body: JSON.stringify({ name, cwd, config_dir: configDir ?? null }),
  });
}

// Web Push: chave VAPID publica deste servidor (applicationServerKey). Vazia = push desligado la.
export async function getVapidKey(s: Server): Promise<string> {
  const res = await fetch(`${s.baseUrl}/api/push/vapid`, {
    headers: { Authorization: `Bearer ${s.token}` },
  });
  if (!res.ok) throw new Error(`vapid ${res.status}`);
  return ((await res.json()).key ?? '') as string;
}

// Registra a inscricao push do celular NESTE servidor, com label + id locais (pra notif e deep-link).
export async function subscribePush(s: Server, subscription: PushSubscriptionJSON): Promise<void> {
  const res = await fetch(`${s.baseUrl}/api/push/subscribe`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ subscription, label: s.label, serverId: s.id }),
  });
  if (!res.ok) throw new Error(`subscribe ${res.status}`);
}

// Preferencias de push (feature #5): sessoes silenciadas + janela de quiet hours global — do servidor
// ATIVO (mesma convencao das ops por-sessao, que sempre miram o server selecionado no momento).
export function getPushSettings(): Promise<{ muted: string[]; quiet_hours: { start: string; end: string } | null }> {
  return apiFetch('/api/push/settings');
}

export function setSessionMute(session: string, muted: boolean): Promise<{ ok: boolean }> {
  return apiFetch('/api/push/mute', { method: 'POST', body: JSON.stringify({ session, muted }) });
}

export function setQuietHours(start: string | null, end: string | null): Promise<{ ok: boolean }> {
  return apiFetch('/api/push/quiet-hours', { method: 'POST', body: JSON.stringify({ start, end }) });
}

export async function deleteSession(name: string): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// Renomeia a sessao do tmux. Devolve o nome final (sanitizado pelo backend).
export async function renameSession(name: string, newName: string): Promise<{ ok: boolean; name: string }> {
  return apiFetch<{ ok: boolean; name: string }>(`/api/sessions/${encodeURIComponent(name)}/rename`, {
    method: 'POST',
    body: JSON.stringify({ new: newName }),
  });
}

// Relança uma sessão "sem id" com `claude --resume <uuid>` -> passa a rastreá-la, continuando a
// conversa. sessionId ausente = deixa o backend escolher (caso seguro) ou devolver candidatos (ambíguo).
export function resumeSession(name: string, sessionId?: string): Promise<ResumeResult> {
  return apiFetch<ResumeResult>(`/api/sessions/${encodeURIComponent(name)}/resume`, {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId ?? null }),
  });
}

// Encadeamento de sessao (feature #12): arma o vinculo 'then' — quando `name` terminar o turno,
// `text` e enviado pra `target` (mesmo backend/servidor; ver app.chain no backend). Um hop so.
export function setThenLink(name: string, target: string, text: string): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/then`, {
    method: 'PUT',
    body: JSON.stringify({ target, text }),
  });
}

export function clearThenLink(name: string): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/then`, {
    method: 'DELETE',
  });
}

// Abre o cwd da sessao no editor da MAQUINA do backend (so-desktop). Binario fixo (CP_EDITOR).
export function openEditor(name: string): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/open-editor`, {
    method: 'POST',
  });
}

export function getHistory(name: string): Promise<ChatEvent[]> {
  return apiFetch<ChatEvent[]>(`/api/sessions/${encodeURIComponent(name)}/history`);
}

export function getCommands(name: string): Promise<CommandInfo[]> {
  return apiFetch<CommandInfo[]>(`/api/sessions/${encodeURIComponent(name)}/commands`);
}

// Workflows: lista de runs + detalhe (fases + agentes) — lidos dos arquivos do run no disco.
export function getWorkflows(name: string): Promise<WorkflowSummary[]> {
  return apiFetch<WorkflowSummary[]>(`/api/sessions/${encodeURIComponent(name)}/workflows`);
}

export function getWorkflow(name: string, runId: string): Promise<WorkflowDetail> {
  return apiFetch<WorkflowDetail>(`/api/sessions/${encodeURIComponent(name)}/workflows/${encodeURIComponent(runId)}`);
}

export function getWorkflowAgent(name: string, runId: string, agentId: string): Promise<WorkflowAgentDetail> {
  return apiFetch<WorkflowAgentDetail>(`/api/sessions/${encodeURIComponent(name)}/workflows/${encodeURIComponent(runId)}/agents/${encodeURIComponent(agentId)}`);
}

// Raízes liberadas do scanner (chips no topo do FolderScanner).
export function getRoots(): Promise<FsRoot[]> {
  return apiFetch<FsRoot[]>('/api/fs/roots');
}

/**
 * Lista os subdiretórios imediatos de `path` (default = `root`) dentro da raiz.
 * Rejeições de fronteira do backend (403 raiz não liberada, 400 caminho inválido,
 * 404 ausente) viram um FsScanResult com `error` tipado: a UI tem UM caminho de
 * renderização (lê `result.error`), em vez de misturar throws com campos. Apenas 401
 * borbulha (problema de auth, não de varredura).
 */
export async function scanDir(root: string, path?: string): Promise<FsScanResult> {
  const qs = new URLSearchParams({ root });
  if (path) qs.set('path', path);
  try {
    return await apiFetch<FsScanResult>(`/api/fs/scan?${qs.toString()}`);
  } catch (e) {
    if (!(e instanceof Error)) throw e;
    const status = parseInt(e.message, 10);
    if (status === 401) throw e;
    const map: Record<number, FsScanError> = {
      400: 'invalid_path',
      403: 'root_not_allowed',
      404: 'not_found',
    };
    return { entries: [], error: map[status] ?? 'unknown' };
  }
}

// ── Arquivo: conversas mortas (transcripts sem sessão tmux viva) ──────────────
// Navegação pasta-primeiro: nível 1 = pastas (agregado barato), nível 2 = conversas da pasta.
export interface ArchiveFolder {
  project: string;
  cwd: string | null;
  count: number;
  mtime: number;
}

export interface ArchiveEntry {
  project: string;
  cwd: string | null;
  session_id: string;
  mtime: number;
  preview: string;
  live: boolean;
}

export function getArchive(): Promise<ArchiveFolder[]> {
  return apiFetch<ArchiveFolder[]>('/api/archive');
}

export function getArchiveFolder(project: string): Promise<ArchiveEntry[]> {
  return apiFetch<ArchiveEntry[]>(`/api/archive/${encodeURIComponent(project)}`);
}

// "Retomar conversa": sobe uma sessao tmux NOVA no cwd original com `claude --resume <uuid>`,
// continuando esta conversa morta. Devolve a SessionInfo da sessao nova (o front navega pro chat dela).
export function resumeArchivedConversation(project: string, sessionId: string): Promise<SessionInfo> {
  return apiFetch<SessionInfo>(
    `/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sessionId)}/resume`,
    { method: 'POST' },
  );
}

export function getArchiveHistory(project: string, sid: string): Promise<ChatEvent[]> {
  return apiFetch<ChatEvent[]>(
    `/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sid)}/history`,
  );
}

// URL de imagem colada no terminal, versão arquivo (mesmo ?token das outras URLs de <img>).
export function archiveImageUrl(project: string, sid: string, id: string, idx: number): string {
  const t = getToken() ?? '';
  return `${getBaseUrl()}/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sid)}/transcript-image/${encodeURIComponent(id)}/${idx}?token=${encodeURIComponent(t)}`;
}

// ── Busca de conteudo cross-session (feature #10): grep (rg) em todos os transcripts do servidor ──
export interface SearchHit {
  project: string;
  session_id: string;
  session_name: string | null;  // nome tmux se a sessao esta viva -> abre o chat; null = arquivo
  cwd: string | null;
  line: string;                 // trecho legivel (texto da msg) ja capado no backend
  mtime: number;
  live: boolean;
}

// Busca em UM servidor (baseUrl+token explicitos), sem mexer no ativo — a UI faz fan-out por servidor
// (mesmo padrao de fetchSessionsForServer) e junta os resultados. Timeout 4s: server morto falha rapido.
export async function searchTranscriptsForServer(s: Server, q: string): Promise<SearchHit[]> {
  const res = await fetch(`${s.baseUrl}/api/search?q=${encodeURIComponent(q)}`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(4000),
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<SearchHit[]>;
}

export async function sendInput(name: string, text: string): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/input`, {
    method: 'POST',
    body: JSON.stringify({ text }),
  });
}

// Fan-out de um prompt pra N sessoes DO SERVIDOR ATIVO (feature #9). Mira sempre 1 servidor por
// chamada — selecao cross-server manda 1 chamada por servidor (selectServer antes, igual ao resto
// do app). O backend roda a MESMA sequencia do /input por nome (fila duravel + confirmacao).
export interface BroadcastResult { ok: boolean; error: string | null }
export async function broadcast(names: string[], text: string): Promise<Record<string, BroadcastResult>> {
  const res = await apiFetch<{ results: Record<string, BroadcastResult> }>('/api/broadcast', {
    method: 'POST',
    body: JSON.stringify({ names, text }),
  });
  return res.results;
}

/**
 * Envia os bytes crus de um arquivo (imagem, video, pdf, ...) pra sessao (sem multipart). O backend
 * salva e devolve o path; o app depois manda a legenda + path pelo /input. O filename vai no header
 * X-Filename (percent-encoded) so pra extensao; o nome final e gerado pelo servidor. 401 -> self-heal.
 */
export async function uploadFile(name: string, file: File): Promise<{ path: string }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/upload`, {
    method: 'POST',
    headers: {
      ...authHeaders(),
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'arquivo'),
    },
    body: file,
  });
  await ensureOk(res);
  return res.json() as Promise<{ path: string }>;
}

export async function selectOption(name: string, option: number): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/select`, {
    method: 'POST',
    body: JSON.stringify({ option }),
  });
}

// ── Git pela sessao (cwd da sessao tmux): listar/trocar branch + status/pull ──
export interface BranchInfo {
  current: string | null;
  branches: string[];
  remotes?: string[];  // remotas sem local correspondente (nome curto); trocar pra uma faz o DWIM do switch
  dirty?: boolean;     // working tree suja -> o front avisa antes de trocar (switch carrega mudancas)
}

export function getBranches(name: string): Promise<BranchInfo> {
  return apiFetch<BranchInfo>(`/api/sessions/${encodeURIComponent(name)}/branches`);
}

export function checkoutBranch(name: string, branch: string): Promise<{ current: string; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/checkout`, {
    method: 'POST',
    body: JSON.stringify({ branch }),
  });
}

export type GitAction = 'status' | 'pull' | 'fetch' | 'stash' | 'stash-pop' | 'log';

export function gitAction(name: string, action: GitAction): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git`, {
    method: 'POST',
    body: JSON.stringify({ action }),
  });
}

export interface ChangedFile {
  path: string;
  code: string;      // 2 chars XY do git porcelain: ' M', 'M ', '??', 'A '...
  staged: boolean;
}

export function getChangedFiles(name: string): Promise<{ files: ChangedFile[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/files`);
}

export function getFileDiff(name: string, path: string): Promise<{ path: string; diff: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/diff`, {
    method: 'POST',
    body: JSON.stringify({ path }),
  });
}

// Um commit da view de log. Campos superset (parents/refs) pro detalhe-de-commit e o grafo (fase 2).
export interface GitCommit {
  hash: string;       // hash completo (âncora do grafo + lookup de detalhe)
  short: string;      // hash curto pra exibir
  parents: string[];  // hashes dos parents (vazio no root; 2+ num merge)
  refs: string;       // decoração %D (branches/tags), sem os parênteses; '' se nenhuma
  author: string;
  ts: number;         // author date, unix epoch (ordenação estável)
  rel: string;        // data relativa pronta ("2 hours ago")
  subject: string;
  col?: number;       // coluna (lane) do commit no grafo — preenchida por assign_lanes no backend
  edges?: { to_col: number; curved: boolean }[];  // arestas descendo pros parents (merge = curva)
  passthrough?: number[];  // colunas de outras lanes que cruzam esta linha sem dot (vertical cheia)
}

export function getGitLog(name: string): Promise<{ commits: GitCommit[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/log`);
}

export function discardFile(name: string, path: string): Promise<{ ok: boolean; path: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/discard`, {
    method: 'POST',
    body: JSON.stringify({ path }),
  });
}

// Envia respostas do stepper AskUserQuestion para o backend.
export function answerQuestions(name: string, answers: AnswerItem[]): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/answer`, {
    method: 'POST', body: JSON.stringify({ answers }),
  });
}

// clear=true tambem limpa o input do terminal (2o Esc no backend). So passar quando havia msg pendente.
export async function interrupt(name: string, clear = false): Promise<void> {
  const q = clear ? '?clear=true' : '';
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/interrupt${q}`, {
    method: 'POST',
    body: JSON.stringify({}),
  });
}

// Espelho do pane (overlays so-TUI): le o pane cru e manda teclas de navegacao (allowlist no backend).
export type NavKey =
  | 'Up' | 'Down' | 'Left' | 'Right'
  | 'Enter' | 'Escape' | 'Tab' | 'BTab'
  | 'PageUp' | 'PageDown' | 'Space';

export async function getPane(name: string): Promise<string> {
  const res = await apiFetch<{ text: string }>(`/api/sessions/${encodeURIComponent(name)}/pane`);
  return res.text;
}

export async function sendKey(name: string, key: NavKey): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/keys`, {
    method: 'POST',
    body: JSON.stringify({ key }),
  });
}

// Terminal interativo (desktop): texto digitado (literal) e/ou tecla nomeada (allowlist no backend).
export async function sendTermInput(name: string, payload: { text?: string; key?: string }): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/term-input`, {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export interface ModelEffortBody {
  model?: string; // 'default' | 'opus' | 'sonnet' | 'haiku'
  effort?: string; // low | medium | high | xhigh | max | ultracode
  scope: 'session' | 'default';
}

/**
 * Applies a model/effort switch by driving Claude Code's interactive `/model` picker.
 * scope 'session' presses `s` (current session only); 'default' presses Enter (saved default).
 * Unlike the old full-arg `/model <arg>` command, scope 'session' does NOT change the user's
 * default for new sessions.
 */
export async function setModelEffort(name: string, body: ModelEffortBody): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/model-effort`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/**
 * Opens an SSE stream for the given session.
 * In production (same-origin), the auth cookie is sent automatically.
 * In dev, appends ?token= as fallback.
 */
export function openEventStream(name: string): EventSource {
  const base = getBaseUrl();
  const token = getToken();
  const path = `/api/sessions/${encodeURIComponent(name)}/events`;

  // Use ?token param only in dev (different origin) or when no cookie is set
  const isSameOrigin = !base || base === window.location.origin;
  const url = isSameOrigin
    ? `${base}${path}`
    : `${base}${path}?token=${encodeURIComponent(token ?? '')}`;

  return new EventSource(url, { withCredentials: isSameOrigin });
}

// EventSource da LISTA de UM servidor (baseUrl/token explícitos). ?token cross-origin (EventSource
// não manda header e cross-origin não leva cookie); withCredentials same-origin. Por-servidor:
// cada um tem o seu, falha isolada.
export function openSessionsStream(s: Server): EventSource {
  const isSameOrigin = !s.baseUrl || s.baseUrl === window.location.origin;
  const url = isSameOrigin
    ? `${s.baseUrl}/api/sessions/events`
    : `${s.baseUrl}/api/sessions/events?token=${encodeURIComponent(s.token)}`;
  return new EventSource(url, { withCredentials: isSameOrigin });
}

// EventSource de UMA sessão de um servidor ESPECÍFICO (baseUrl/token explícitos, sem tocar no
// ativo) — usado pela grade de comparação (feature #11), que pode misturar sessões de servidores
// diferentes no mesmo relance. Mesma convenção de openSessionsStream (?token cross-origin,
// withCredentials same-origin).
export function openEventStreamForServer(s: Server, name: string): EventSource {
  const path = `/api/sessions/${encodeURIComponent(name)}/events`;
  const isSameOrigin = !s.baseUrl || s.baseUrl === window.location.origin;
  const url = isSameOrigin
    ? `${s.baseUrl}${path}`
    : `${s.baseUrl}${path}?token=${encodeURIComponent(s.token)}`;
  return new EventSource(url, { withCredentials: isSameOrigin });
}

// ── Preview: expõe um projeto local (porta) via `tailscale serve` da máquina do backend, pra ver
// num iframe. Global por máquina (slot único), não por sessão.
export interface PreviewState {
  active: boolean;
  port: number | null;
  url: string | null;
}

export function getPreview(): Promise<PreviewState> {
  return apiFetch<PreviewState>('/api/preview');
}

export function startPreview(port: number): Promise<{ url: string; port: number }> {
  return apiFetch('/api/preview', { method: 'POST', body: JSON.stringify({ port }) });
}

export function stopPreview(): Promise<PreviewState> {
  return apiFetch('/api/preview', { method: 'DELETE' });
}

export function getRunners(name: string): Promise<RunnersResponse> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/runners`);
}

export function startRun(name: string, command: string): Promise<RunInfo> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/run`, {
    method: 'POST',
    body: JSON.stringify({ command }),
  });
}

export function stopRun(name: string): Promise<{ ok: boolean }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/run/stop`, { method: 'POST' });
}

export function getRunPane(name: string): Promise<{ pane: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/run/pane`);
}
