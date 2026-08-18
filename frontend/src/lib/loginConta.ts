// Cliente do login remoto numa conta (Task 7): começar, ler o passo, confirmar o código,
// cancelar. O módulo da casa (lib/api.ts) está fechado neste lote — cada módulo novo cria o
// seu fetch no padrão de servidor explícito (mesmo de lib/peers.ts). Este módulo é a ÚNICA
// porta do front pro login: a tela (ContasSettings.svelte) não chama fetch direto.
import { getBaseUrl, getToken, dropActiveServer, type Server } from './auth';
import { errorDetail, isTimeoutError } from './api';
import * as m from '../paraglide/messages';

// Erros de rede: "Failed to fetch" cru nao chega a tela (regra da casa: e.message cru e
// ingles de navegador numa tela em portugues). A excecao e o 401 com token (sessao expirada,
// tratado abaixo) e o TETO de tempo (TimeoutError, que o usuario precisa entender).

export interface PassoLogin {
  etapa: 'idle' | 'aguardando';
  url?: string | null;
}

export interface ResultadoLogin {
  ok: boolean;
  email?: string | null;
  plano?: string | null;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  let res: Response;
  try {
    res = await fetch(`${getBaseUrl()}${path}`, {
      ...init,
      headers: {
        'Content-Type': 'application/json',
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...(init?.headers ?? {}),
      },
    });
  } catch (e) {
    // Teto estourado lança DE DENTRO do fetch, antes de qualquer resposta — o ramo antigo que
    // checava `res.status === 0` era código morto (fetch resolvido nunca tem status 0). O teto
    // é falha de verdade e tentar de novo tem valor: frase da casa traduzida, nunca o
    // 'signal timed out' cru (a mesma regra do cabeçalho: e.message cru não chega à tela).
    if (isTimeoutError(e)) throw Object.assign(new Error(m.erro_login_timeout()), { status: 0 });
    throw e;
  }
  // Mesmo contrato do api.ts: 401 com token salvo = credencial velha, deslogar e voltar pro
  // Login. 401 sem token (sessão do app expirada no servidor) vira erro de rede comum.
  if (res.status === 401 && token) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw Object.assign(new Error(m.sessao_expirada()), { status: 401 });
  }
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json() as Promise<T>;
}

// Servidor EXPLÍCITO (a aba aponta pra outra máquina). Nunca faz self-heal: um 401 aqui é erro
// DAQUELE servidor e não pode apagar a credencial ativa, que é de outra máquina (mesmo desenho do
// lib/peers.ts e do apiFetchForServer). Prazo de 8s POR PADRÃO: servidor atrás de VPN não recusa
// conexão, pendura — sem prazo o login ficava "aguardando" pra sempre. `timeoutMs` existe porque
// um teto único não serve a duas chamadas deste módulo: o confirmarLogin espera o backend segurar
// o laço do OAuth por até 300s (_TIMEOUT_S) e passa 310_000 (precedente: getHistory, api.ts).
async function reqEm<T>(s: Server, path: string, init?: RequestInit, timeoutMs = 8000): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${s.baseUrl}${path}`, {
      signal: AbortSignal.timeout(timeoutMs),
      ...init,
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${s.token}`,
        ...(init?.headers ?? {}),
      },
    });
  } catch (e) {
    if (isTimeoutError(e)) throw Object.assign(new Error(m.erro_login_timeout()), { status: 0 });
    throw e;
  }
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json() as Promise<T>;
}

// Uma porta só pros exportados: alvo null = servidor ativo (é o contrato do apiTarget). O
// timeoutMs só vale no caminho explícito — o do ativo não tem teto (é o comportamento de sempre).
function em<T>(alvo: Server | null, path: string, init?: RequestInit, timeoutMs = 8000): Promise<T> {
  return alvo ? reqEm<T>(alvo, path, init, timeoutMs) : req<T>(path, init);
}

export function iniciarLogin(alvo: Server | null, conta: string): Promise<{ ok: boolean }> {
  return em(alvo, `/api/conta-estado/${encodeURIComponent(conta)}/login`, { method: 'POST' });
}

export function passoLogin(alvo: Server | null, conta: string): Promise<PassoLogin> {
  return em(alvo, `/api/conta-estado/${encodeURIComponent(conta)}/login/passo`);
}

export function confirmarLogin(alvo: Server | null, conta: string, codigo: string): Promise<ResultadoLogin> {
  return em(alvo, `/api/conta-estado/${encodeURIComponent(conta)}/login/codigo`, {
    method: 'POST',
    body: JSON.stringify({ codigo }),
  }, 310_000);
}

export function cancelarLogin(alvo: Server | null, conta: string): Promise<{ ok: boolean }> {
  return em(alvo, `/api/conta-estado/${encodeURIComponent(conta)}/login/cancelar`, { method: 'POST' });
}
