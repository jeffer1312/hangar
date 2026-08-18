// Cliente da rota /api/conta-estado (fonte única do estado das contas — a Task 9 consome a
// mesma rota para a faixa de cota; a Task 7 liga o botão Entrar por cima deste shape).
//
// lib/api.ts está fechado neste lote: o fetch segue o MESMO padrão dele (authHeaders +
// ensureOk com 401 → dropActiveServer + reload), sem tocar no arquivo. `errorDetail` vem de
// api.ts, que é exportado — importar não é editar.
import { getBaseUrl, getToken, dropActiveServer, type Server } from './auth';
import { errorDetail } from './api';
import * as m from '../paraglide/messages';

export type EstadoLogin = {
  estado: 'ok' | 'indisponivel';
  loggedIn?: boolean | null;
  email?: string | null;
  plano?: string | null; // subscriptionType cru ("max"/"pro"/...) — dado do servidor
  motivo?: string | null;
};

export type EstadoLimite = {
  estado: 'lido' | 'sem_leitura';
  linha?: string | null; // linha inteira da statusline — a Task 9 parseia com lib/statusline
  ts?: number | null;
  idade_s?: number | null;
};

export interface ContaEstado {
  path: string;
  label: string;
  active: boolean;
  login: EstadoLogin;
  limite: EstadoLimite;
}

// Mesmo par do lib/peers.ts (dd863b88): `req` fala com o servidor ATIVO (self-heal de 401),
// `reqEm` com um servidor EXPLÍCITO — sem self-heal, porque um 401 de outra máquina não pode
// apagar a credencial ativa (mesmo contrato de api.ts:129-131). Prazo de 8s no explícito:
// servidor atrás de VPN não recusa conexão, pendura — sem prazo a lista ficava "Carregando…".
async function req<T>(path: string): Promise<T> {
  const token = getToken();
  const res = await fetch(`${getBaseUrl()}${path}`, {
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
  // Mesmo contrato do ensureOk do api.ts: 401 com token salvo = credencial velha, deslogar.
  if (res.status === 401 && token) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw Object.assign(new Error(m.sessao_expirada()), { status: 401 });
  }
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json();
}

async function reqEm<T>(s: Server, path: string): Promise<T> {
  const res = await fetch(`${s.baseUrl}${path}`, {
    signal: AbortSignal.timeout(8000),
    headers: { Authorization: `Bearer ${s.token}` },
  });
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json();
}

// Uma porta só pros exportados: alvo null = servidor ativo (é o contrato do apiTarget).
function em<T>(alvo: Server | null, path: string): Promise<T> {
  return alvo ? reqEm<T>(alvo, path) : req<T>(path);
}

export async function listarEstadosDeConta(alvo: Server | null): Promise<ContaEstado[]> {
  return em<ContaEstado[]>(alvo, '/api/conta-estado');
}

// --------------------------------------------------------------------------- cota por conta
//
// A cota NÃO sai daqui do estado de conta: /api/cotas fala com o provedor usando a credencial
// de cada conta (ver backend/app/cotas.py). O motivo é medido — o `limite` acima vem do sidecar
// de statusline DENTRO da pasta da conta, e numa máquina onde essa pasta é um symlink pra conta
// padrão as três contas liam o MESMO arquivo. Sessão parada também nunca teve leitura nenhuma.
// O fetch é o mesmo par req/reqEm (401 do servidor ativo desloga; do explícito, não).

export type EstadoCota = 'lida' | 'sem_credencial' | 'expirada' | 'indisponivel';

export interface JanelaCota {
  /** Rótulo da janela como o PROVEDOR o define ("5h"/"7d") — dado do servidor, não interface. */
  rotulo: string;
  pct: number;
  reset_ts?: number | null;
}

export interface CotaConta {
  id: string;
  label: string;
  provedor: 'claude' | 'kimi';
  ativa: boolean;
  estado: EstadoCota;
  janelas: JanelaCota[];
  ts?: number | null;
  idade_s?: number | null;
  motivo?: string | null;
}

export async function listarCotas(alvo: Server | null): Promise<CotaConta[]> {
  return em<CotaConta[]>(alvo, '/api/cotas');
}

// Intervalo curto ("2 h", "3 d") para a idade de uma leitura — a frase completa vem das
// chaves cota_lido_agora / cota_ultima_leitura ({n} recebe este intervalo). Formato de DADO
// (unidade abrevidada, como o mock "última leitura há 2 h"), não frase de interface.
// Comentário trailing com aspas conta como string crua no i18nScan (resíduo conhecido) — por
// isso as notas vão em linha própria, nunca no fim do código.
export function formatarIntervalo(s: number | null | undefined): string {
  if (s == null || !isFinite(s)) return '—';
  // "1 min" é o piso: "lido agora" cobre menos de um minuto nas chaves cota_*.
  if (s < 60) return '1 min';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86400)} d`;
}

