// Cliente do módulo de peers (backend/app/peers_api.py): máquinas que este servidor alcança,
// e o identificador desta máquina. A CREDENCIAL nunca volta inteira — o backend devolve
// mascarada; este módulo só exibe. Módulo próprio porque o cliente da casa (lib/api.ts) está
// fechado para as Tasks deste plano — cada uma cria o seu, no mesmo padrão.
import { dropActiveServer, getBaseUrl, getToken } from './auth';
import { errorDetail } from './api';
import * as m from '../paraglide/messages';

/** Peer como o backend devolve: `token` é SEMPRE a máscara (conferível, não copiável). */
export interface PeerView {
  id: string;
  base_url: string;
  token: string;
  web_url?: string | null;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const res = await fetch(`${getBaseUrl()}${path}`, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(init?.headers ?? {}),
    },
  });
  // Mesmo self-heal do apiFetch: 401 com token salvo = credencial morta, volta pro Login.
  if (res.status === 401 && token) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw Object.assign(new Error(m.sessao_expirada()), { status: 401 });
  }
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json() as Promise<T>;
}

export function listarPeers(): Promise<PeerView[]> {
  return req<PeerView[]>('/api/peers');
}

export function gravarPeer(dado: { id: string; base_url: string; token: string; web_url?: string }): Promise<PeerView[]> {
  return req<PeerView[]>('/api/peers', { method: 'POST', body: JSON.stringify(dado) });
}

export function removerPeer(id: string): Promise<PeerView[]> {
  return req<PeerView[]>(`/api/peers/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export function getIdentificador(): Promise<{ identificador: string }> {
  return req<{ identificador: string }>('/api/peers/identificador');
}

export function setIdentificador(identificador: string): Promise<{ identificador: string }> {
  return req<{ identificador: string }>('/api/peers/identificador', {
    method: 'PUT',
    body: JSON.stringify({ identificador }),
  });
}