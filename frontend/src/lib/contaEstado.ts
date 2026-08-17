// Cliente da rota /api/conta-estado (fonte única do estado das contas — a Task 9 consome a
// mesma rota para a faixa de cota; a Task 7 liga o botão Entrar por cima deste shape).
//
// lib/api.ts está fechado neste lote: o fetch segue o MESMO padrão dele (authHeaders +
// ensureOk com 401 → dropActiveServer + reload), sem tocar no arquivo. `errorDetail` vem de
// api.ts, que é exportado — importar não é editar.
import { getBaseUrl, getToken, dropActiveServer } from './auth';
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

export async function listarEstadosDeConta(): Promise<ContaEstado[]> {
  const token = getToken();
  const res = await fetch(`${getBaseUrl()}/api/conta-estado`, {
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

