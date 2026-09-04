// Cliente de /api/credenciais — a lista ÚNICA da aba Contas: conta do Claude (login) e chave de
// API na MESMA tabela, cada uma com apelido e limite.
//
// Só duas operações moram aqui: LER a lista e trocar o APELIDO. Criar, apagar e editar continuam
// nas rotas que já existiam (`/api/claude-configs` pra conta do Claude, `/api/engines/{nome}` pra
// chave) — unificar a tela não pode significar dois donos do mesmo dado no servidor.
//
// O fetch segue o mesmo par do contaEstado.ts: `null` = servidor ATIVO (401 desloga), Server
// explícito = máquina do ?srv= (401 de outra máquina não pode apagar a credencial ativa).
import { getBaseUrl, getToken, dropActiveServer, type Server } from './auth';
import { errorDetail } from './api';
import * as m from '../paraglide/messages';
import type { EstadoLogin } from './contaEstado';
import type { JanelaCota, EstadoCota } from './contaEstado';

export type TipoCredencial = 'claude' | 'chave';

export interface CotaResumo {
  estado: EstadoCota;
  janelas: JanelaCota[];
  ts?: number | null;
  idade_s?: number | null;
  motivo?: string | null;
}

export interface Credencial {
  id: string;
  tipo: TipoCredencial;
  /** O que a tela mostra: o apelido, quando existe. */
  nome: string;
  /** O que o disco diz (nome da pasta / do motor). É ele que vai nas rotas de escrita. */
  nome_natural: string;
  apelido?: string | null;
  ativa: boolean;
  path?: string | null;
  login?: EstadoLogin | null;
  base_url?: string | null;
  chave_mascarada?: string | null;
  usos: string[];
  cota?: CotaResumo | null;
  /** Só o OpenCode: a cota dele não sai de API, sai do painel com o cookie de sessão. */
  aceita_cookie?: boolean;
  cookie_definido?: boolean;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const res = await fetch(`${getBaseUrl()}${path}`, {
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

async function reqEm<T>(s: Server, path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${s.baseUrl}${path}`, {
    ...init,
    signal: AbortSignal.timeout(8000),
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      Authorization: `Bearer ${s.token}`,
    },
  });
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json();
}

function em<T>(alvo: Server | null, path: string, init?: RequestInit): Promise<T> {
  return alvo ? reqEm<T>(alvo, path, init) : req<T>(path, init);
}

// `forcar` é o botão "atualizar" da aba: pede ao servidor a leitura de cota de AGORA,
// pulando o cache de 5 min (ver backend/app/cotas.py — `?forcar=true`).
export function listarCredenciais(alvo: Server | null, forcar = false): Promise<Credencial[]> {
  return em<Credencial[]>(alvo, `/api/credenciais${forcar ? '?forcar=true' : ''}`);
}

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
