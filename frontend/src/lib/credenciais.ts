// Cliente de /api/credenciais — a lista ÚNICA da aba Contas: conta do Claude (login) e chave de
// API na MESMA tabela, cada uma com apelido e limite.
//
// Só duas operações moram aqui: LER a lista e trocar o APELIDO. Criar, apagar e editar continuam
// nas rotas que já existiam (`/api/claude-configs` pra conta do Claude, `/api/engines/{nome}` pra
// chave) — unificar a tela não pode significar dois donos do mesmo dado no servidor.
//
// O fetch segue o mesmo par do contaEstado.ts: `null` = servidor ATIVO (401 desloga), Server
// explícito = máquina do ?srv= (401 de outra máquina não pode apagar a credencial ativa).
import { getRouteBaseUrl, getToken, dropActiveServer, type Server } from './auth';
import { baseOf, consumeCodexRateLimitReset, consumeCodexRateLimitResetForServer, errorDetail, comTeto,
  type CodexAccount, type CodexResetOutcome } from '@hangar/core';
import * as m from '../paraglide/messages';
export { listarCredenciais, credentialAuth, credentialGroup, codexAccountMessage } from '@hangar/core';
export type { Credencial, CotaResumo, TipoCredencial, AuthMethod, CodexAccount, CodexLoginAttempt } from '@hangar/core';

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const res = await fetch(`${getRouteBaseUrl()}${path}`, {
    ...init,
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
  });
  if (res.status === 401 && token) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw Object.assign(new Error(m.sessao_expirada()), { status: 401 });
  }
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json();
}

async function reqEm<T>(s: Server, path: string, init?: RequestInit, prazoMs = 8000): Promise<T> {
  const res = await fetch(`${baseOf(s)}${path}`, {
    ...init,
    signal: comTeto(init?.signal ?? undefined, prazoMs),
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      Authorization: `Bearer ${s.token}`,
    },
  });
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json();
}

function em<T>(alvo: Server | null, path: string, init?: RequestInit, prazoMs?: number): Promise<T> {
  return alvo ? reqEm<T>(alvo, path, init, prazoMs) : req<T>(path, init);
}

export { codexOpcoes, type CodexOpcoes } from '@hangar/core';

export function novaChaveIdempotente(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function consumirRedefinicaoCodex(
  alvo: Server | null, conta: string, credito: string | null, chaveIdempotente: string,
): Promise<{ outcome: CodexResetOutcome }> {
  return alvo
    ? consumeCodexRateLimitResetForServer(alvo, conta, credito, chaveIdempotente)
    : consumeCodexRateLimitReset(conta, credito, chaveIdempotente);
}

// `forcar` é o botão "atualizar" da aba: pede ao servidor a leitura de cota de AGORA,
// pulando o cache de 5 min (ver backend/app/cotas.py — `?forcar=true`).

/** Apelido vazio APAGA o apelido (volta ao nome do disco). */
export function definirApelido(
  alvo: Server | null, id: string, apelido: string,
): Promise<{ id: string; apelido: string | null }> {
  return em(alvo, '/api/credenciais/apelido', {
    method: 'PUT',
    body: JSON.stringify({ id, apelido }),
  });
}

// Guarda (ou apaga, com os dois vazios) o cookie do painel do OpenCode desta credencial.
// A resposta traz só o booleano — o cookie é sessão de navegador e não volta pra tela.
// (Comentário em `//`: o extrator do i18nGuard lê bloco `/** */` de várias linhas como texto de
// interface. Falso positivo, mas escrever assim é mais barato que uma exceção nomeada.)
export function definirCookie(
  alvo: Server | null, id: string, workspace_id: string, auth_cookie: string,
): Promise<{ id: string; cookie_definido: boolean }> {
  return em(alvo, '/api/credenciais/cookie', {
    method: 'PUT',
    body: JSON.stringify({ id, workspace_id, auth_cookie }),
  });
}

export interface ResultadoSync {
  id: string;
  /** Quantos modelos o provedor listou na hora da gravação (0 = provedor sem /v1/models). */
  modelos: number;
  resultado: Record<string, { ok: boolean; motivo: string }>;
}

// Grava a credencial na configuração dos OUTROS agentes (Pi, Kimi, Codex). O Claude Code não entra
// na lista porque ele já É o engines.json — a credencial nasce gravada lá.
// Só o id viaja: a chave o servidor já tem, e mandá-la de novo poria o segredo num corpo de request
// que a tela guarda em memória sem precisar.
export function sincronizarNosAgentes(alvo: Server | null, id: string): Promise<ResultadoSync> {
  return em(alvo, '/api/credenciais/sincronizar', {
    method: 'POST',
    body: JSON.stringify({ id }),
  });
}

// Login OAuth do ChatGPT (o do Codex), feito pelo servidor por código de dispositivo e espalhado
// pro Codex, Pi e omp. O front só mostra o código e faz o poll do passo.
export interface EstadoCodex {
  cofre: boolean;
  plano: string;
  expira_em: number | null;
  codex: boolean;
  pi: boolean;
  omp: boolean;
}

export interface PassoCodex {
  etapa: 'idle' | 'aguardando' | 'concluido' | 'falhou' | 'cancelado';
  user_code?: string;
  url?: string;
  erro?: string;
  resultado?: Record<string, { ok: boolean; motivo: string }> | null;
}

export function codexEstado(alvo: Server | null): Promise<EstadoCodex> {
  return em(alvo, '/api/credenciais/codex');
}

export function codexLoginIniciar(alvo: Server | null): Promise<PassoCodex> {
  return em(alvo, '/api/credenciais/codex/login', { method: 'POST' });
}

export function codexLoginPasso(alvo: Server | null): Promise<PassoCodex> {
  return em(alvo, '/api/credenciais/codex/login');
}

export function codexLoginCancelar(alvo: Server | null): Promise<PassoCodex> {
  return em(alvo, '/api/credenciais/codex/login', { method: 'DELETE' });
}

// Painel de saúde dos harnesses (backend/app/harness_saude.py): o que o app instalou em cada CLI
// e o botão que refaz. O texto de cada item vem como código + params e é traduzido aqui.
export interface ItemHarness {
  id: string;
  ok: boolean | null;
  codigo: string;
  params: Record<string, string>;
  conserto: string | null;
  /** Linha informativa (o que o CLI tem), não checagem: ponto em vez de ✓, nunca pinta o card. */
  info?: boolean;
}

export interface Harness {
  id: string;
  nome: string;
  instalado: boolean;
  versao: string | null;
  itens: ItemHarness[];
}

export function listarHarnesses(alvo: Server | null): Promise<Harness[]> {
  return em(alvo, '/api/harness');
}

export function consertarHarness(alvo: Server | null, conserto: string): Promise<{ feito: string; harnesses: Harness[] }> {
  return em(alvo, `/api/harness/conserto/${encodeURIComponent(conserto)}`, { method: 'POST' });
}

// MCP hangar-computer-control. As chaves nunca voltam inteiras: só se existem e o final.
export interface ComputerControlTarget { name: string; path: string; transport: string; host: string }

export interface ComputerControlState {
  agent_exe: { path: string; exists: boolean; size: number };   // windows-agent.exe desta máquina
  mode: 'package' | 'local';      // package = instalado pelo botão (uvx + release); local = pasta com o código
  installed_tag: string;
  package_exists: boolean;
  targets: ComputerControlTarget[];
  ssh_hosts: string[];              // os Host do ~/.ssh/config do servidor
  local_available: boolean;   // o servidor é Windows: a própria máquina pode ser alvo
  enabled: boolean;
  project_dir: string;
  agent_config: string;
  agent_configs: string[];
  llm_url: string;
  llm_model: string;
  llm_effort: string;
  llm_key_set: boolean;
  llm_key_tail: string;
  jev_key_set: boolean;
  jev_key_tail: string;
  jev_endpoint?: string;   // configuração única do Jev, só leitura aqui
  jev_model?: string;
  cliproxy: { preset_url: string; has_keys: boolean; key_is_cliproxy: boolean; installed: boolean; running: boolean };
  files: { path: string; enabled: boolean }[];
  migration_skipped?: string[];   // só no retorno do install: alvos da pasta local que não deu pra ler
}

export interface ComputerControlRequest {
  enabled: boolean;
  mode?: 'package' | 'local';
  project_dir: string;
  agent_config: string;
  llm_url: string;
  llm_model: string;
  llm_effort: string;
  llm_key?: string | null;
  use_cliproxy_key?: boolean;
}

export function getComputerControl(target: Server | null): Promise<ComputerControlState> {
  return em(target, '/api/computer-control');
}

export function saveComputerControl(target: Server | null, request: ComputerControlRequest): Promise<ComputerControlState> {
  return em(target, '/api/computer-control', { method: 'PUT', body: JSON.stringify(request) });
}

// Baixa a release: pode levar mais que o teto padrão de 8 s de uma chamada a outro servidor.
export function installComputerControl(target: Server | null): Promise<ComputerControlState> {
  return em(target, '/api/computer-control/install', { method: 'POST' }, 180000);
}

export function testComputerControlHost(target: Server | null, request: { host: string; proxy_command?: string }):
  Promise<{ ok: boolean; detail: string }> {
  return em(target, '/api/computer-control/test-host', { method: 'POST', body: JSON.stringify(request) }, 30000);
}

export function getComputerControlWindowsSetup(target: Server | null, host: string): Promise<{ prompt: string; user: string }> {
  return em(target, `/api/computer-control/windows-setup?host=${encodeURIComponent(host)}`);
}

export function createComputerControlTarget(target: Server | null, request: {
  project_dir: string; name: string; transport: 'ssh' | 'local'; host?: string; proxy_command?: string;
  request_timeout?: number | null;
}): Promise<ComputerControlState> {
  return em(target, '/api/computer-control/targets', { method: 'POST', body: JSON.stringify(request) });
}

export function listComputerControlModels(target: Server | null, request: {
  llm_url: string; llm_key?: string | null; use_saved_key?: boolean; use_cliproxy_key?: boolean;
}): Promise<{ models: string[] }> {
  return em(target, '/api/computer-control/models', { method: 'POST', body: JSON.stringify(request) });
}

// Código + parâmetros vêm do backend (`codex_msgs.CATALOGO`); o front traduz por `harness_codex_m_<codigo>`.
// `texto` é o fallback em pt para código que este app ainda não conhece. String crua = backend antigo.
export type MensagemCodex = { codigo: string | null; params: Record<string, string>; texto: string } | string;

export interface IntegracaoCodex {
  estado: 'ocioso' | 'executando' | 'ok' | 'parcial' | 'erro' | 'indisponivel';
  etapa: MensagemCodex | null;
  ultima_execucao: string | null;
  proxima_atualizacao: string | null;
  plugins: { id: string; versao: string; origem: string }[];
  erros: MensagemCodex[];
  avisos: MensagemCodex[];
  confianca_pendente: boolean;
  automatica: boolean;
  memoria: boolean;
  skills?: { ponte: number; nativas: number };
  /** Só com a rodada andando: etapa X de N (fixas no servidor) e, dentro dela, item i de N. */
  progresso?: { passo: number; total: number; sub: { atual: number; total: number } | null } | null;
  etapa_segundos?: number | null;
}

export { comTeto } from '@hangar/core';

export function codexIntegracaoEstado(alvo: Server | null, signal?: AbortSignal): Promise<IntegracaoCodex> {
  return em(alvo, '/api/harness/codex/integracao', { signal: comTeto(signal, 8000) });
}

export function codexIntegracaoReconciliar(alvo: Server | null, signal?: AbortSignal): Promise<IntegracaoCodex> {
  return em(alvo, '/api/harness/codex/integracao', { method: 'POST', signal: comTeto(signal, 8000) });
}

export function listarContasCodex(alvo: Server | null, signal?: AbortSignal): Promise<CodexAccount[]> {
  return em(alvo, '/api/codex-contas', { signal: comTeto(signal, 8000) });
}

export function prepararContaCodex(
  alvo: Server | null, id: string, forcar = false, signal?: AbortSignal,
): Promise<CodexAccount['sync']> {
  const query = forcar ? '?forcar=true' : '';
  return em(alvo, `/api/codex-contas/${encodeURIComponent(id)}/prepare${query}`,
    { method: 'POST', signal: comTeto(signal, 8000) });
}

export function estadoContaCodex(
  alvo: Server | null, id: string, signal?: AbortSignal,
): Promise<CodexAccount['sync']> {
  return em(alvo, `/api/codex-contas/${encodeURIComponent(id)}/prepare`,
    { signal: comTeto(signal, 8000) });
}

// Instalar um CLI que falta (backend/app/harness_install.py). `comandos` diz o que dá pra instalar
// por botão NESTA máquina — quem não está lá só tem o link de `manual`.
export interface Instalacao {
  fase: 'ocioso' | 'rodando' | 'pronto';
  harness: string | null;
  /** Chave da etapa; a frase é daqui (`harness_inst_etapa_<etapa>`). */
  etapa: string | null;
  passo: number;
  total: number;
  log: string[];
  /** Etapa pulada com motivo legítimo (sem bash no Windows): não é falha, mas não pode ficar no log. */
  avisos?: string[];
  ok: boolean | null;
  erro: string | null;
  comandos: Record<string, string>;
  manual: Record<string, string>;
}

export function instalacaoEstado(alvo: Server | null, signal?: AbortSignal): Promise<Instalacao> {
  return em(alvo, '/api/harness/instalar', { signal: comTeto(signal, 8000) });
}

export function instalarHarness(alvo: Server | null, cli: string, signal?: AbortSignal): Promise<Instalacao> {
  return em(alvo, `/api/harness/instalar/${encodeURIComponent(cli)}`, { method: 'POST', signal: comTeto(signal, 8000) });
}
