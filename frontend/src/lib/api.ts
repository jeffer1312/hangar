import { getBaseUrl, getToken, dropActiveServer } from './auth';
import type { Server } from './auth';
import type {
  SessionInfo,
  Provider,
  ChatEvent,
  CommandInfo,
  ConfigDirInfo,
  FsRoot,
  FsScanResult,
  FsScanError,
  WorkflowSummary,
  SubagentRun,
  WorkflowDetail,
  WorkflowAgentDetail,
  AnswerItem,
  CostReport,
  ResumeResult,
  RunnersResponse,
  RunInfo,
  SessionLimits,
  CodexModelsResponse,
  PiModelsResponse,
  LoopState,
  UploadFile,
  PlanDetail,
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

// URL do mp3 gerado. `?token` porque <audio> nao manda header Authorization e o front vem de outra
// origem (PWA servido pela VPS, backend no Tailscale) — cookie tambem nao viaja.
export function ttsAudioUrl(path: string): string {
  const t = getToken() ?? '';
  return `${getBaseUrl()}${path}?token=${encodeURIComponent(t)}`;
}

function authHeaders(): HeadersInit {
  const token = getToken();
  if (!token) return {};
  return { Authorization: `Bearer ${token}` };
}

// Mensagem de erro legivel a partir do corpo de uma resposta !ok. FastAPI devolve {"detail": "..."}
// -> extrai a mensagem limpa em vez do JSON cru (esse texto vai direto pra UI). Fallbacks: corpo
// nao-JSON vira o texto cru; corpo vazio/ilegivel cai no res.statusText. Compartilhado pelo ensureOk
// (caminho do servidor ativo) e pelas funcoes *ForServer, pra que o MESMO 404 do backend produza a
// MESMA string nos dois caminhos.
async function errorDetail(res: Response): Promise<string> {
  const text = await res.text().catch(() => '');
  try {
    const j = JSON.parse(text);
    if (j && typeof j.detail === 'string') return j.detail;
  } catch { /* corpo nao-JSON: cai no texto cru abaixo */ }
  // text e statusText podem os DOIS vir vazios (502 de infra sem corpo JSON, servidor HTTP/2 que
  // nao popula statusText) — sem este ultimo fallback, quem le `.message` (TtsBar, ServerSettings)
  // trata string vazia como "sem erro" e desenha a UI de sucesso por cima de uma falha real.
  return text || res.statusText || `falha ${res.status} sem detalhe do servidor`;
}

// Trata a resposta compartilhada por apiFetch e uploadFile. Self-heal de token invalido/rotacionado:
// isAuthenticated() so checa se EXISTE token, nao se vale. Num 401 COM token salvo, limpamos a
// credencial e recarregamos -> cai no Login pra re-parear (QR). O guard getToken() evita loop quando
// ja estamos deslogados (Login nao chama a API). Qualquer outro !ok vira erro com o corpo.
async function ensureOk(res: Response): Promise<void> {
  if (res.status === 401 && getToken()) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw Object.assign(new Error('sessão expirada — faça login novamente'), { status: 401 });
  }
  // `status` no proprio erro: sem ele quem chama (ex: ouvir.ts) nao consegue distinguir um 409
  // (acima do limite de aviso, pede confirmacao) de qualquer outra falha so pela mensagem. A
  // MENSAGEM fica limpa (sem o "409: " na frente) — quem precisa do numero le `.status`, nao
  // texto que o usuario acaba vendo cru (ex: window.confirm da confirmacao de custo do TTS).
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
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

// Configurações abertas a partir da visão agregada precisam continuar no servidor capturado, sem
// trocar o servidor global. Um 401 aqui é erro local da sheet: nunca remove a credencial ativa,
// que pode pertencer a outra máquina.
async function apiFetchForServer<T>(s: Server, path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${s.baseUrl}${path}`, {
      // Prazo por PADRAO. Esta funcao fala com OUTRO servidor, e servidor offline atras de VPN nao
      // recusa a conexao — o socket fica pendurado e a promessa nunca resolve (o comentario do
      // getSessions ja registrava isso pro poll). Sem prazo, abrir Configuracoes de um servidor
      // desligado prendia a folha em "Carregando..." pra sempre, sem erro nenhum na tela.
      // Antes do spread do `init`: quem precisar de outro prazo (ou de nenhum) passa o proprio sinal.
      signal: AbortSignal.timeout(8000),
      ...init,
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${s.token}`,
        ...(init?.headers ?? {}),
      },
    });
  } catch (e) {
    // "signal timed out" (o texto que o navegador poe no TimeoutError) nao diz nada pra quem le a
    // tela. Abort pedido POR QUEM CHAMOU continua passando cru — quem cancela sabe que cancelou.
    if (e instanceof DOMException && e.name === 'TimeoutError') {
      throw new Error(`${s.label} não respondeu em 8s — servidor fora do ar?`);
    }
    throw e;
  }
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<T>;
}

export function getSessions(): Promise<SessionInfo[]> {
  // Timeout curto: este alimenta polls (ex. nav do Chat a cada 5s) — socket pendurado (tailscale
  // pra nó morto nao recusa) empilhava um fetch por tick até esgotar as 6 conexões do host.
  return apiFetch<SessionInfo[]>('/api/sessions', { signal: AbortSignal.timeout(4000) });
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
// `period` OBRIGATÓRIO: o servidor ecoa em `applied` o período que aplicou, e o merge recusa
// somar quem não ecoou o pedido. Deixar o parâmetro opcional é convidar o chamador a esquecê-lo,
// receber de volta o default do backend e jogar a malha INTEIRA em `mismatched` — a tela diria
// "todos os servidores desatualizados" quando o bug é do front.
// Devolve `Partial<CostReport>` porque é isto que chega DO FIO: um servidor da malha em versão
// antiga responde sem os campos novos, e prometer o objeto completo aqui é como o front
// quebrava em runtime com o `check` verde.
export async function fetchCostsForServer(s: Server, period: string): Promise<Partial<CostReport>> {
  const res = await fetch(`${s.baseUrl}/api/costs?period=${encodeURIComponent(period)}`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(4000),
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<Partial<CostReport>>;
}

// Cauda do histórico de UMA sessão de um servidor específico — cards do quadro kanban.
// limit dispara o tail-read no backend (parseia só o fim do jsonl). Timeout de 8s mantido: disco
// frio + arquivo grande ainda pode passar dos 4s dos fan-outs acima.
export async function getHistoryTailForServer(s: Server, name: string, limit: number): Promise<ChatEvent[]> {
  const res = await fetch(
    `${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/history?limit=${limit}`,
    { headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` }, signal: AbortSignal.timeout(8000) },
  );
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<ChatEvent[]>;
}

// Cache de cauda pros cards do board/canvas. Vive no MÓDULO de propósito: trocar de view no
// DesktopShell desmonta board/canvas inteiros, então um cache em componente morreria junto — e é
// exatamente a troca de view que dispara a tempestade de 50 GET /history. A chave inclui STATE e
// LAST_ACTIVITY: só state deixava buraco — ciclo idle→working→idle completo dentro do TTL voltava
// pra MESMA chave e servia a cauda pré-troca como atual; last_activity (mtime do jsonl, re-emitido
// pelo stream de lista a cada mudança real) muda junto com conteúdo novo e fura o cache na volta.
// `at` volta pro chamador: o retire de eco pendente do BoardCard só pode aposentar echos
// confirmados ANTES da cauda ser buscada (não do hit) — senão msg entregue some da UI até o
// próximo fetch real. `evs` sai como CÓPIA rasa: o chamador joga o array num $state (proxy sobre a
// própria referência) — devolver o array do cache criaria aliasing e uma mutação futura no
// componente corromperia a entrada pros demais consumidores da chave.
const _tailCache = new Map<string, { at: number; evs: ChatEvent[] }>();
const _TAIL_TTL = 30_000;

export async function getHistoryTailCached(
  s: Server, name: string, limit: number, state: string, lastActivity: number | null | undefined,
): Promise<{ evs: ChatEvent[]; at: number }> {
  const key = `${s.id}::${name}::${state}::${lastActivity ?? 0}::${limit}`;
  const hit = _tailCache.get(key);
  if (hit && Date.now() - hit.at < _TAIL_TTL) return { at: hit.at, evs: [...hit.evs] };
  const entry = { at: Date.now(), evs: await getHistoryTailForServer(s, name, limit) };
  _tailCache.set(key, entry);
  if (_tailCache.size > 300) {
    for (const [k, v] of _tailCache) if (Date.now() - v.at >= _TAIL_TTL) _tailCache.delete(k);
  }
  return { at: entry.at, evs: [...entry.evs] };
}

// Upload/transcrição pra sessão de um servidor específico — o composer COMPLETO do card do
// board/canvas (mesmos endpoints/headers dos uploadFile/transcribeFile do servidor ativo; aqui
// baseUrl+token vêm do Server dono do card, que pode não ser o ativo).
export async function uploadFileForServer(s: Server, name: string, file: File): Promise<{ path: string }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/upload`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${s.token}`,
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'arquivo'),
    },
    body: file,
    // Sem teto, uma foto grande num link ruim (tablet em relay) deixava o composer preso em
    // "enviando…" pra sempre. 3min cobre upload legítimo lento; estourou -> erro visível + retry.
    signal: AbortSignal.timeout(180_000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ path: string }>;
}

export async function transcribeFileForServer(s: Server, name: string, file: File): Promise<{ path: string; text: string }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/transcribe`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${s.token}`,
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'audio.webm'),
    },
    body: file,
    signal: AbortSignal.timeout(120_000),   // mesmo teto do transcribeFile: sem "transcrevendo…" eterno
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ path: string; text: string }>;
}

// Envia prompt pra sessão de um servidor específico (input do card do quadro). 404 = sessão morta:
// o chamador REMOVE o eco pendente e sinaliza — mensagem nunca "some" calada (mesmo contrato do
// feedback de entrega do Chat). SEM timeout de propósito (igual ao sendInput por-servidor-ativo):
// abortar um POST já em voo não desfaz o envio, e reportaria "não entregue" pra recado entregue.
export async function sendInputForServer(s: Server, name: string, text: string): Promise<void> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/input`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ text }),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
}

// Responde uma opção do picker (awaiting_input) direto do card. Mesma convenção de índice do
// selectOption por-servidor-ativo (api.ts): option é 1-BASED (1 = primeira opção) — o backend
// valida ge=1 e traduz pra (option-1)×Down + Enter no tmux.
export async function selectOptionForServer(s: Server, name: string, option: number): Promise<void> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/select`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ option }),
  });
  // Mesmo tratamento do sendInputForServer: o erro do picker tambem e renderizado no card.
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
}

export function listClaudeConfigs(): Promise<ConfigDirInfo[]> {
  return apiFetch<ConfigDirInfo[]>('/api/claude-configs');
}

export function createSession(
  name: string,
  cwd?: string,
  configDir?: string | null,
  provider: Provider = 'claude',
  engine?: string | null,
): Promise<SessionInfo> {
  return apiFetch<SessionInfo>('/api/sessions', {
    method: 'POST',
    body: JSON.stringify({ name, cwd, config_dir: configDir ?? null, provider, engine: engine ?? null }),
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

export function getPushSettingsForServer(s: Server): Promise<{ muted: string[]; quiet_hours: { start: string; end: string } | null }> {
  return apiFetchForServer(s, '/api/push/settings');
}

export function setSessionMute(session: string, muted: boolean): Promise<{ ok: boolean }> {
  return apiFetch('/api/push/mute', { method: 'POST', body: JSON.stringify({ session, muted }) });
}

export function setQuietHours(start: string | null, end: string | null): Promise<{ ok: boolean }> {
  return apiFetch('/api/push/quiet-hours', { method: 'POST', body: JSON.stringify({ start, end }) });
}

export function setQuietHoursForServer(s: Server, start: string | null, end: string | null): Promise<{ ok: boolean }> {
  return apiFetchForServer(s, '/api/push/quiet-hours', { method: 'POST', body: JSON.stringify({ start, end }) });
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

// Cancelamento NÃO é falha. O Chat aborta o /history em voo quando a carga perde a validade (troca
// de sessão, /clear, volta do background, unmount) — o rejeito que vem daí não pode virar pílula de
// erro na tela. `TimeoutError` (o teto de 45s abaixo) fica de FORA de propósito: aquilo é falha de
// verdade e o usuário precisa ver.
export function isAbortError(e: unknown): boolean {
  return e instanceof Error && e.name === 'AbortError';
}

// Histórico da sessão ATIVA. Com `limit` é a MESMA rota do tail dos cards do board
// (getHistoryTailForServer): o backend faz tail-read, parseando só o fim do jsonl em vez do
// arquivo inteiro. O Chat abre com a cauda e busca o resto (sem limit) em segundo plano.
// `signal` cancela de verdade: sem ele o fetch do histórico COMPLETO (medido: 1596 eventos num
// jsonl de 136MB) seguia baixando depois de já ter sido descartado — banda e parse à toa no
// celular, e vários em paralelo quando o usuário pula de sessão em sessão.
export function getHistory(name: string, limit?: number, signal?: AbortSignal): Promise<ChatEvent[]> {
  // Teto largo (transcript grande em link lento existe), mas TETO: o resume do iOS chamava isto
  // sem timeout e um socket pendurado deixava o fetch em voo por minutos, sobrescrevendo estado
  // novo com foto velha quando enfim resolvia.
  const cap = AbortSignal.timeout(45_000);
  // `!== undefined` e não truthy: limit=0 é um pedido explícito de zero, não um pedido do arquivo inteiro.
  const q = limit !== undefined ? `?limit=${limit}` : '';
  return apiFetch<ChatEvent[]>(`/api/sessions/${encodeURIComponent(name)}/history${q}`, {
    signal: signal ? AbortSignal.any([signal, cap]) : cap,
  });
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

// Subagentes soltos (tool Agent) da sessão: o transcript PRÓPRIO de cada um, que o jsonl do pai
// não carrega. É o que permite ver as ferramentas que ele está chamando enquanto roda.
export function getSubagents(name: string): Promise<SubagentRun[]> {
  return apiFetch<SubagentRun[]>(`/api/sessions/${encodeURIComponent(name)}/subagents`);
}
export function getSubagent(name: string, agentId: string, events = 0): Promise<SubagentRun> {
  const q = events ? `?events=${events}` : '';
  return apiFetch<SubagentRun>(`/api/sessions/${encodeURIComponent(name)}/subagents/${encodeURIComponent(agentId)}${q}`);
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
    // `.status`, nao parseInt(e.message): ensureOk (api.ts) parava de embutir o status no TEXTO
    // da mensagem, entao ler o numero de la quebraria toda vez que o detail do backend comecasse
    // com digito (ex: "404 arquivos encontrados"). O status ja vem anotado no proprio erro.
    const status = (e as Error & { status?: number }).status ?? NaN;
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
// engine: o pane original morreu, entao nao ha /proc pra descobrir que motor rodava -- quem retoma
// escolhe de novo (ou nenhum -> volta na conta Anthropic, igual hoje). Body vazio quando omitido:
// o backend aceita `ResumeArchivedBody = ResumeArchivedBody()` como default, entao chamadores antigos
// continuam funcionando sem mandar nada.
export function resumeArchivedConversation(
  project: string,
  sessionId: string,
  engine?: string | null,
): Promise<SessionInfo> {
  return apiFetch<SessionInfo>(
    `/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sessionId)}/resume`,
    { method: 'POST', body: JSON.stringify({ engine: engine ?? null }) },
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
// RAG lexical "onde falei sobre X": o backend busca trechos (rg) e um claude -p efemero responde
// apontando as sessoes. Timeout largo (90s): busca + chamada de modelo.
export async function askHistoryForServer(
  s: Server,
  question: string,
): Promise<{ answer: string; hits: SearchHit[] }> {
  const res = await fetch(`${s.baseUrl}/api/ask-history`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ question }),
    signal: AbortSignal.timeout(90000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json();
}

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

// Pareamento ("trabalhando juntas"): o backend grava o vínculo simétrico e injeta o prompt de
// pareamento nas DUAS sessões — daí em diante elas se falam via cp-send por iniciativa própria.
// warning: falha PARCIAL de aviso (algum membro sem o prompt do grupo) — o backend reporta de
// propósito; descartar isso virava "sucesso" mudo com membro que não sabe que está no grupo.
export interface PairResult { ok: boolean; warning: string | null }
export async function pairSession(name: string, peers: string[], task = ''): Promise<PairResult> {
  return apiFetch<PairResult>(`/api/sessions/${encodeURIComponent(name)}/pair`, {
    method: 'POST',
    body: JSON.stringify({ peers, task }),
  });
}

export async function unpairSession(name: string): Promise<PairResult> {
  return apiFetch<PairResult>(`/api/sessions/${encodeURIComponent(name)}/pair`, {
    method: 'DELETE',
  });
}

// Contrato compartilhado do par: markdown que as duas sessões editam via fs; o app só exibe.
export interface PairContract { peers: string[]; path: string; content: string }
export function getPairContract(name: string): Promise<PairContract> {
  return apiFetch<PairContract>(`/api/sessions/${encodeURIComponent(name)}/pair/contract`);
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
// Lista os anexos JA enviados pra sessao (galeria). Sem timeout curto de propósito: roda sob
// interação do usuário (abrir a sheet), não em poll — falhar rápido aqui só viraria erro à toa.
export function listUploads(name: string): Promise<{ files: UploadFile[] }> {
  return apiFetch<{ files: UploadFile[] }>(`/api/sessions/${encodeURIComponent(name)}/uploads`);
}

// ── Configuração do servidor ────────────────────────────────────────────────
// O segredo (chave da Groq) volta MASCARADO — dá pra conferir qual chave está lá, não pra copiar.
export interface CampoConfig {
  valor: string | number | boolean | null;
  definido: boolean;
  origem: 'app' | 'env';
}
export interface ConfigServidor {
  campos: Record<string, CampoConfig>;
  // `terminal_panel` (Task 6, Step 8) e o unico booleano aqui -- `pty` e POSIX-only.
  somente_leitura: Record<string, string | number | boolean>;
}

export function getConfig(): Promise<ConfigServidor> {
  return apiFetch('/api/config');
}

export function getConfigForServer(s: Server): Promise<ConfigServidor> {
  return apiFetchForServer(s, '/api/config');
}

export function patchConfig(mudancas: Record<string, unknown>): Promise<{ campos: Record<string, CampoConfig> }> {
  // POST, nao PATCH: o proxy na frente do backend barra PATCH (era o unico do app).
  return apiFetch('/api/config', { method: 'POST', body: JSON.stringify(mudancas) });
}

export function patchConfigForServer(s: Server, mudancas: Record<string, unknown>): Promise<{ campos: Record<string, CampoConfig> }> {
  return apiFetchForServer(s, '/api/config', { method: 'POST', body: JSON.stringify(mudancas) });
}

// Detalhe do plano em execução (Task/Step, markdown cru). 404 = sem plano ativo, NÃO é erro — o
// chamador (PlanPanel) trata null como "nada pra mostrar", não como falha. Por isso um fetch cru
// em vez de apiFetch/apiFetchForServer: as duas lançam pra qualquer !ok, inclusive 404.
export async function getPlan(name: string): Promise<PlanDetail | null> {
  const res = await fetch(`${getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/plan`, {
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<PlanDetail>;
}

// Um plano do repo, pro seletor. Inclui os NÃO começados (0/N) e os completos, que a eleição
// automática descarta — são justamente os que o usuário precisa poder escolher na mão.
export interface PlanListItem {
  stem: string;      // nome do arquivo sem .md — é o que o pin grava
  name: string;      // rótulo já sem o prefixo de data
  done: number;
  total: number;
  complete: boolean;
}

export function getPlans(name: string): Promise<{ plans: PlanListItem[]; pinned: string | null }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/plans`);
}

// stem = null solta o pin e devolve o painel pra eleição automática.
export function setPlanPin(name: string, stem: string | null): Promise<{ pinned: string | null }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/plan-pin`, {
    method: 'POST', body: JSON.stringify({ stem }),
  });
}

// ── Motores de modelo ───────────────────────────────────────────────────────
export interface Motor {
  label?: string;
  base_url: string;
  model: string;
  subagent_model?: string;
  context_window?: number;
  vision?: boolean | null;
  // Capacidades do harness. Sempre positivas ("true = ligado"); o backend traduz pras env vars
  // negativas (DISABLE_*) em engines.env_de.
  tool_search?: boolean;
  bundled_skills?: boolean;
  experimental_betas?: boolean;
  prompt_caching?: boolean;
  adaptive_thinking?: boolean;
  gateway_model_discovery?: boolean;
  fine_grained_tool_streaming?: boolean;
  // Manda a chave também no header `x-api-key` (além do `Authorization: Bearer`). Provedor que só
  // lê o primeiro devolve `401 Missing API key` sem isto — ver o snippet mAuthHeader na tela.
  auth_via_api_key?: boolean;
  auto_compact_window?: number;
  max_output_tokens?: number;
  // Sempre mascarada (sk-k••••••••1234). A chave inteira nunca volta do servidor.
  api_key: string;
  api_key_definida: boolean;
}
export interface ModeloProvedor {
  id: string;
  context_length: number | null;
  vision: boolean | null;
}

export interface EnginesResponse {
  motores: Record<string, Motor>;
  // true quando engines.json existe mas não pôde ser lido (hand-edit quebrado, etc): a tela
  // precisa distinguir isto de "nenhum motor configurado" — as duas batem em `motores: {}`.
  arquivo_corrompido: boolean;
  arquivo_caminho: string;
}

export function getEngines(): Promise<EnginesResponse> {
  return apiFetch('/api/engines');
}

export function getEnginesForServer(s: Server): Promise<EnginesResponse> {
  return apiFetchForServer(s, '/api/engines');
}

export function putEngine(nome: string, dados: Record<string, unknown>): Promise<{ motores: Record<string, Motor> }> {
  return apiFetch(`/api/engines/${encodeURIComponent(nome)}`, {
    method: 'PUT',
    body: JSON.stringify(dados),
  });
}

export function putEngineForServer(s: Server, nome: string, dados: Record<string, unknown>): Promise<{ motores: Record<string, Motor> }> {
  return apiFetchForServer(s, `/api/engines/${encodeURIComponent(nome)}`, {
    method: 'PUT',
    body: JSON.stringify(dados),
  });
}

export function deleteEngine(nome: string): Promise<{ ok: boolean }> {
  return apiFetch(`/api/engines/${encodeURIComponent(nome)}`, { method: 'DELETE' });
}

export function deleteEngineForServer(s: Server, nome: string): Promise<{ ok: boolean }> {
  return apiFetchForServer(s, `/api/engines/${encodeURIComponent(nome)}`, { method: 'DELETE' });
}

// Modelos que a key pode usar, direto do provedor. Também é o "Testar": erro aqui traz a mensagem
// do provedor (401, host errado), em vez de deixar o usuário sem pista.
// `nome` OU `base_url`+`api_key` — nunca os dois (o servidor rejeita com 400, pra key salva nunca
// viajar pra um endereço que o cliente digitou).
type EngineModelosBody = { nome: string } | { base_url: string; api_key: string };

export function engineModelos(corpo: EngineModelosBody): Promise<{ modelos: ModeloProvedor[] }> {
  return apiFetch('/api/engines/modelos', { method: 'POST', body: JSON.stringify(corpo) });
}

export function engineModelosForServer(s: Server, corpo: EngineModelosBody): Promise<{ modelos: ModeloProvedor[] }> {
  return apiFetchForServer(s, '/api/engines/modelos', { method: 'POST', body: JSON.stringify(corpo) });
}

export async function uploadFile(
  name: string,
  file: File,
): Promise<{ path: string; frames?: string[]; transcript?: string }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/upload`, {
    method: 'POST',
    headers: {
      ...authHeaders(),
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'arquivo'),
    },
    body: file,
    // Mesmo teto do uploadFileForServer (o fix de ontem cobriu só a variante do board; esta é a
    // do chat principal — auditoria achou o composer preso em "enviando…" por aqui também).
    signal: AbortSignal.timeout(180_000),
  });
  await ensureOk(res);
  return res.json() as Promise<{ path: string }>;
}

/**
 * Envia os bytes de um audio (gravado no mic ou arquivo) pra sessao. O backend salva o audio E o
 * transcreve via Groq num round-trip, devolvendo { path, text }. O app monta a mensagem
 * "<transcricao> — 📎 audio: <path>". Mesmo esquema de header (X-Filename) do uploadFile.
 * `limpar: true` (so o ditado pelo mic, nunca arquivo anexado) pede `?limpar=1` — o backend limpa o
 * texto ANTES de responder (sem corrida/troca na tela) e devolve tambem `raw` (pro desfazer) e
 * `aviso` (motivo da limpeza nao ter valido, ou null quando valeu).
 */
export async function transcribeFile(
  name: string,
  file: File,
  opts?: { limpar?: boolean },
): Promise<{ path: string; text: string; raw?: string; aviso?: string | null }> {
  const base = getBaseUrl();
  const qs = opts?.limpar ? '?limpar=1' : '';
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/transcribe${qs}`, {
    method: 'POST',
    headers: {
      ...authHeaders(),
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'audio.webm'),
    },
    body: file,
    // timeout (mesmo teto do backend): rede travada rejeita em vez de deixar o composer preso em
    // "transcrevendo…" pra sempre (transcribing nunca voltaria a false).
    signal: AbortSignal.timeout(120_000),
  });
  await ensureOk(res);
  return res.json() as Promise<{ path: string; text: string; raw?: string; aviso?: string | null }>;
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

export type GitAction = 'status' | 'pull' | 'fetch' | 'stash' | 'stash-pop' | 'log'
  | 'revert-abort' | 'cherry-pick-abort';

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

// `sequencer`: revert/cherry-pick em andamento (conflito ainda nao resolvido/abortado), lido do
// DISCO (CHERRY_PICK_HEAD/REVERT_HEAD) — nao de memoria de sessao. E o que permite o botao de
// abort sobreviver a um reload/reabertura da sheet enquanto o repo continua em conflito.
export function getChangedFiles(name: string): Promise<{ files: ChangedFile[]; sequencer: 'revert' | 'cherry-pick' | null }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/files`);
}

export function getFileDiff(name: string, path: string): Promise<{ path: string; diff: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/diff`, {
    method: 'POST',
    body: JSON.stringify({ path }),
  });
}

export function getCommitFiles(name: string, sha: string): Promise<{ files: ChangedFile[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/files`);
}

export function getCommitFileDiff(name: string, sha: string, path: string): Promise<{ path: string; diff: string }> {
  const q = new URLSearchParams({ path });
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff?${q}`);
}

// Diff unificado do commit INTEIRO (todos os arquivos) — a "Show changes as unified diff" do Tortoise.
// `truncated`: o backend capa em 200KB (_DIFF_MAX em git_ops.py) — precisa chegar na UI, senao um
// diff cortado parece completo e uma decisao (ex. reset --hard) seria tomada em cima de metade dele.
export function getCommitDiff(name: string, sha: string): Promise<{ sha: string; diff: string; truncated: boolean }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff-full`);
}

export function gitRevert(name: string, sha: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/revert`, {
    method: 'POST', body: JSON.stringify({ sha }),
  });
}

export function gitCherryPick(name: string, sha: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/cherry-pick`, {
    method: 'POST', body: JSON.stringify({ sha }),
  });
}

export type GitResetMode = 'soft' | 'mixed' | 'hard';

export function gitReset(name: string, sha: string, mode: GitResetMode): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/reset`, {
    method: 'POST', body: JSON.stringify({ sha, mode }),
  });
}

export function gitCreateBranch(name: string, opts: { name: string; sha?: string; switch_after?: boolean }): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/branch`, {
    method: 'POST', body: JSON.stringify(opts),
  });
}

export function gitCreateTag(name: string, opts: { name: string; sha?: string; message?: string }): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/tag`, {
    method: 'POST', body: JSON.stringify(opts),
  });
}

// Commit vs o DISCO agora — o "Compare with working tree" do Tortoise. Mesmo teto/`truncated` do
// getCommitDiff (git_ops.py:_cap aplica aos dois).
export function getCommitDiffVsWorktree(name: string, sha: string): Promise<{ sha: string; diff: string; truncated: boolean }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff-worktree`);
}

// Branches (locais e remotas) que contêm o commit.
export function getCommitBranches(name: string, sha: string): Promise<{ local: string[]; remote: string[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/branches`);
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
  body: string;       // corpo da mensagem (%b), sem o assunto; '' quando o commit nao tem corpo
  col?: number;       // coluna (lane) do commit no grafo — preenchida por assign_lanes no backend
  edges?: { to_col: number; curved: boolean }[];  // arestas descendo pros parents (merge = curva)
  passthrough?: number[];  // colunas de outras lanes que cruzam esta linha sem dot (vertical cheia)
}

export function getGitLog(name: string, q?: string): Promise<{ commits: GitCommit[] }> {
  const qs = q ? `?q=${encodeURIComponent(q)}` : '';
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/log${qs}`);
}

export function discardFile(name: string, path: string): Promise<{ ok: boolean; path: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/discard`, {
    method: 'POST',
    body: JSON.stringify({ path }),
  });
}

export function commitFiles(name: string, message: string, paths: string[],
                            opts?: { amend?: boolean; newBranch?: string }): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/commit`, {
    method: 'POST',
    body: JSON.stringify({ message, paths, amend: opts?.amend ?? false, new_branch: opts?.newBranch ?? null }),
  });
}

// Mensagem completa do HEAD (pra pré-preencher o amend). 409 se o repo não tem commit.
export function getLastCommitMessage(name: string): Promise<{ message: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/last-message`);
}

export function gitPush(name: string): Promise<{ ok: boolean; output: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/git/push`, { method: 'POST' });
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

// `lines` = quanto scrollback trazer acima da tela visível (o espelho pede mais ao rolar pro topo).
// `scrollback` na resposta = quantas linhas o tmux REALMENTE tem; vale 0 num TUI de tela alternada
// (Claude Code), onde pedir mais nunca traz nada e subir é papel do PageUp do próprio TUI.
export async function getPane(name: string, lines?: number): Promise<{ text: string; scrollback: number }> {
  const qs = lines ? `?lines=${lines}` : '';
  const res = await apiFetch<{ text: string; scrollback?: number }>(
    `/api/sessions/${encodeURIComponent(name)}/pane${qs}`);
  return { text: res.text, scrollback: res.scrollback ?? 0 };
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

// Cria (ou reata) a sessao de shell escondida do app, no cwd da sessao do agente. Devolve o nome
// tmux real ("term-<nome>") — e ele, nao `name`, que a aba do shell usa pra conectar. 409 = ja
// existe uma sessao com esse nome que nao e o nosso shell; a mensagem do backend explica o motivo,
// e quem chama deve mostra-la (nao engolir).
export function openShell(name: string): Promise<{ ok: true; shell: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/shell`, { method: 'POST' });
}

// Abre um emulador de terminal NATIVO (janela do SO) anexado a sessao `name`. 503 = sem emulador no
// PATH, ou o emulador morreu logo apos abrir — o `detail` do erro e texto pra humano, mostrar direto.
export function openNativeTerminal(name: string): Promise<{ ok: true }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/open-terminal`, { method: 'POST' });
}

export interface ModelEffortBody {
  model?: string; // keyword de uma linha do picker: 'default' | 'opus' | 'fable' | 'sonnet' | …
  effort?: string; // low | medium | high | xhigh | max | ultracode
  scope: 'session' | 'default';
}

export interface ModelOption {
  id: string;              // keyword do picker (conta) ou id do provedor (motor)
  name?: string;           // rotulo exibido (so no picker da conta)
  desc?: string;           // descricao da linha do picker
  active?: boolean;
  context_length?: number | null;  // so no motor
  vision?: boolean | null;         // so no motor
}

export interface ModelOptionsResponse {
  kind: 'claude' | 'engine';
  engine: string | null;
  effort?: string | null;
  models: ModelOption[];
}

/**
 * Modelos que ESTA sessao pode escolher. Nada e chumbado no front de proposito: numa sessao da
 * conta a lista sai do proprio picker do Claude Code (ela muda com a conta e com a versao — o
 * Fable entrou e a lista fixa daqui nao soube), e numa sessao de MOTOR sai do /v1/models do
 * provedor (o picker ali so lista os 4 aliases, todos o mesmo modelo).
 * 409 = sessao ocupada/menu aberto: nao da pra ler o picker agora.
 */
export function getModelOptions(name: string): Promise<ModelOptionsResponse> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/model/options`);
}

/**
 * Troca o modelo de uma sessao de MOTOR (digita `/model <id>`). O backend repoe o default global
 * que esse comando grava de lambuja — a troca vale so nesta sessao.
 */
export function setEngineModel(
  name: string,
  body: { model: string; effort?: string | null },
): Promise<{ ok: boolean; model: string; result: string | null; effort_error?: string }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/engine/model`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
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

// provider: quem RESPONDEU de fato (pode ter virado "local" pelo fallback do backend quando falta
// a chave da ElevenLabs mas ha comando local configurado) — nao necessariamente o que foi pedido.
export interface TtsResposta { url: string; chars: number; cached: boolean; provider: string }

// `confirm: true` repete o pedido depois que o usuario aceitou o aviso de custo (409 do backend).
// `instruction`: fase 2 (narracao guiada) — a instrucao que ja tratou este `text` via /api/tts/narrar
// (ou "" quando foi lido como esta). So entra na chave do cache do backend, nunca dispara a Groq
// aqui: quando isto chega, o texto ja esta pronto pra virar audio.
export async function sintetizarTts(
  body: { text: string; voice?: string; provider?: string; confirm?: boolean; instruction?: string },
): Promise<TtsResposta> {
  return apiFetch<TtsResposta>('/api/tts', { method: 'POST', body: JSON.stringify(body) });
}

export interface TtsVoz { id: string; nome: string }

export async function listarVozesTts(): Promise<TtsVoz[]> {
  const r = await apiFetch<{ voices: TtsVoz[] }>('/api/tts/voices');
  return r.voices;
}

export async function saldoTts(): Promise<{ usados: number | null; limite: number | null }> {
  return apiFetch('/api/tts/saldo');
}

// Fase 2 (narracao guiada): pede pra Groq tratar o texto falavel ANTES de virar audio. `used_groq`
// diz se a chamada de fato saiu (false quando a instrucao e "ler como esta" — o backend nao gasta
// token nesse caminho, e devolve o texto de volta como veio).
export interface NarrarResposta { text: string; chars_sent: number; used_groq: boolean }

export async function narrarSelecao(
  body: { text: string; code_blocks: string[]; instruction: string },
): Promise<NarrarResposta> {
  return apiFetch<NarrarResposta>('/api/tts/narrar', { method: 'POST', body: JSON.stringify(body) });
}

/**
 * Opens an SSE stream for the given session.
 * In production (same-origin), the auth cookie is sent automatically.
 * In dev, appends ?token= as fallback.
 */
// `lastEventId`: posição de retomada do transcript ("<stem-do-jsonl>:<offset>"), enviada como QUERY
// PARAM de propósito. O header `Last-Event-ID` só é reenviado quando o MESMO objeto EventSource se
// reconecta sozinho — e este app nunca deixa isso acontecer: o `onerror` fecha o es e o `connectSSE`
// cria um novo (para o auto-retry nativo não virar uma 2ª máquina de retry em paralelo), e o
// watchdog de 25s faz o mesmo. Objeto novo nasce sem memória de id, então sem este param a retomada
// exata jamais dispararia no uso real, e toda queda voltaria a custar o backfill cego de 200 linhas.
export function openEventStream(name: string, lastEventId?: string | null): EventSource {
  const base = getBaseUrl();
  const token = getToken();
  const path = `/api/sessions/${encodeURIComponent(name)}/events`;

  // Use ?token param only in dev (different origin) or when no cookie is set
  const isSameOrigin = !base || base === window.location.origin;
  const params = new URLSearchParams();
  if (!isSameOrigin) params.set('token', token ?? '');
  if (lastEventId) params.set('last_event_id', lastEventId);
  const qs = params.toString();
  const url = `${base}${path}${qs ? `?${qs}` : ''}`;

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

// Limites de uso da conta Codex (Task B) — so sessoes Codex; o back devolve 400 pra Claude.
export function getLimits(name: string): Promise<SessionLimits> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/limits`);
}

// Modelo + reasoning effort do Codex (Task C) — so sessoes Codex; o back devolve 400 pra Claude.
export function getCodexModels(name: string): Promise<CodexModelsResponse> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/models`);
}

// Grava a escolha (dict + sidecar no backend); vale a partir do PROXIMO turno enviado.
export function setCodexModel(name: string, model: string, effort?: string | null): Promise<void> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/model`, {
    method: 'POST',
    body: JSON.stringify({ model, effort: effort ?? undefined }),
  });
}

// ── Modelo + nivel de raciocinio de uma sessao Pi ─────────────────────────────────────────────
// 409 = extensao cp-state.ts ausente/desatualizada no Pi (o backend manda a instrucao no detail).

export function getPiModels(name: string): Promise<PiModelsResponse> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/pi/models`);
}

// Aplica na sessao viva (digita /cp-model e/ou /cp-think). A resposta e o READ-BACK: o Pi clampa o
// nivel pro que o modelo suporta, entao quem manda no rotulo e o que voltou, nao o que foi pedido.
export function setPiModel(
  name: string,
  body: { provider?: string; model?: string; effort?: string | null },
): Promise<{ ok: boolean; current: PiModelsResponse['current']; thinking: string | null; levels: string[] }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/pi/model`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

// ── Loop runner (Task 9+): obter, criar, parar e resolver loops autonomos por sessao ───────────

// Obtem o estado atual de um loop (ou null se nao existe) e sugestoes de próximas ações.
export async function getLoopForServer(s: Server, name: string): Promise<{ loop: LoopState | null; suggestions: string[] }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/loop`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(8000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState | null; suggestions: string[] }>;
}

// Cria um novo loop com goal, check_cmd opcional, max_iters e require_branch.
export async function createLoopForServer(s: Server, name: string, body: { goal: string; check_cmd?: string | null; max_iters?: number; require_branch?: boolean }): Promise<{ loop: LoopState }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/loop`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState }>;
}

// Para um loop em execucao (muda status para 'stopped').
// Refina o objetivo do loop via claude -p efemero no backend (boas praticas embutidas no prompt).
// Timeout proprio de 60s: o claude -p leva segundos e nao e mutacao — abortar e seguro.
export async function refineLoopForServer(
  s: Server,
  name: string,
  goal: string,
  check_cmd: string | null,
): Promise<{ goal: string }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/loop/refine`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ goal, check_cmd }),
    signal: AbortSignal.timeout(60000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json();
}

export async function stopLoopForServer(s: Server, name: string): Promise<{ loop: LoopState }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/loop`, {
    method: 'DELETE',
    headers: { Authorization: `Bearer ${s.token}` },
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState }>;
}

// Resolve um loop no estado 'done_claimed' (accept=true) ou 'stopped' (accept=false).
export async function resolveLoopForServer(s: Server, name: string, accept: boolean): Promise<{ loop: LoopState }> {
  const res = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/loop/resolve`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ accept }),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState }>;
}
