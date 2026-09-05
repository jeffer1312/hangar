// Cliente do módulo de peers (backend/app/peers_api.py): máquinas que este servidor alcança,
// e o identificador desta máquina. A CREDENCIAL nunca volta inteira — o backend devolve
// mascarada; este módulo só exibe. Módulo próprio porque o cliente da casa (lib/api.ts) está
// fechado para as Tasks deste plano — cada uma cria o seu, no mesmo padrão.
import { dropActiveServer, getBaseUrl, getToken, type Server } from './auth';
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

// Servidor EXPLÍCITO (a aba aponta pra outra máquina). Nunca faz self-heal: um 401 aqui é erro
// DAQUELE servidor e não pode apagar a credencial ativa, que é de outra máquina (mesmo motivo de
// api.ts:129-131). Prazo de 8s: servidor atrás de VPN não recusa conexão, pendura — sem prazo a
// seção ficava "Carregando…" pra sempre (mesmo desenho de apiFetchForServer).
async function reqEm<T>(s: Server, path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${s.baseUrl}${path}`, {
    signal: AbortSignal.timeout(8000),
    ...init,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${s.token}`,
      ...(init?.headers ?? {}),
    },
  });
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.json() as Promise<T>;
}

// Uma porta só pros exportados: alvo null = servidor ativo (é o contrato do apiTarget).
function em<T>(alvo: Server | null, path: string, init?: RequestInit): Promise<T> {
  return alvo ? reqEm<T>(alvo, path, init) : req<T>(path, init);
}

export function listarPeers(alvo: Server | null): Promise<PeerView[]> {
  return em<PeerView[]>(alvo, '/api/peers');
}

export function gravarPeer(alvo: Server | null, dado: { id: string; base_url: string; token: string; web_url?: string }): Promise<PeerView[]> {
  return em<PeerView[]>(alvo, '/api/peers', { method: 'POST', body: JSON.stringify(dado) });
}

export function removerPeer(alvo: Server | null, id: string): Promise<PeerView[]> {
  return em<PeerView[]>(alvo, `/api/peers/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export function getIdentificador(alvo: Server | null): Promise<{ identificador: string }> {
  return em<{ identificador: string }>(alvo, '/api/peers/identificador');
}

export function setIdentificador(alvo: Server | null, identificador: string): Promise<{ identificador: string }> {
  return em<{ identificador: string }>(alvo, '/api/peers/identificador', {
    method: 'PUT',
    body: JSON.stringify({ identificador }),
  });
}

/** Estado de UM lado de um peer (Task 8): o backend devolve o estado nomeado. */
export function checkPeer(alvo: Server | null, url: string, id: string): Promise<{
  estado: 'ok' | 'estranho' | 'falhou' | 'recusou' | 'nao_configurado';
  identificador?: string;
  motivo?: string;
  tempo_ms?: number | null;
}> {
  return em<{
    estado: 'ok' | 'estranho' | 'falhou' | 'recusou' | 'nao_configurado';
    identificador?: string;
    motivo?: string;
    tempo_ms?: number | null;
  }>(alvo, `/api/peers/check?url=${encodeURIComponent(url)}&id=${encodeURIComponent(id)}`);
}

// Desmarcar "servidores se falam" desfaz o registro nas DUAS pontas quando o navegador tem o
// token da outra máquina (é o mesmo par de gravações que registrarPeerDoisLados faz, ao
// contrário). Devolve só o lado de lá: o daqui já aconteceu ou relançou. 404 de lá é "já não
// estava" — o que a pessoa queria.
export async function removerPeerDoisLados(
  dono: Server | null,
  id: string,
  remoto: Server | null,
): Promise<boolean | null> {
  await removerPeer(dono, id);
  if (!remoto) return null;
  try {
    const { identificador } = await getIdentificador(dono);
    if (!identificador) return false;
    await removerPeer(remoto, identificador);
    return true;
  } catch (e) {
    return (e instanceof Error && (e as any).status === 404);
  }
}