import type { Server, VaultPeer } from '@hangar/core';

/** Host, porta e caminho, sem barra final: o mesmo endereço escrito de dois jeitos casa. */
export function hostKey(url: string): string {
  try {
    const u = new URL(url.trim());
    return `${u.host}${u.pathname.replace(/\/+$/, '')}`.toLowerCase();
  } catch { return url.trim().replace(/\/+$/, '').toLowerCase(); }
}

/** Servidor salvo no app que é a dona `peer` (pelo endereço ou pelo id que ela respondeu); `null` se nenhum. */
export function ownerServer(peer: VaultPeer, servers: Server[]): Server | null {
  const want = hostKey(peer.base_url);
  return servers.find((s) => !s.invite && !s.disabled
    && ([s.baseUrl, s.lan?.url, s.remote?.url].some((u) => !!u && hostKey(u) === want)
      || s.lan?.id === peer.id || s.remote?.id === peer.id)) ?? null;
}
