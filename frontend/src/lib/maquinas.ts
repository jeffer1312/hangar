// As duas listas de máquinas — a do navegador (este aparelho acompanha) e a do servidor (peers,
// os servidores se falam) — são donos diferentes e continuam separadas. A tela mostra UMA linha
// por máquina, casando as duas pelo identificador: pela URL viravam duas, porque o celular
// conhece a máquina pelo IP da rede local e o servidor pelo Tailscale.
import type { Server } from './auth';
import type { PeerView } from './peers';

export interface LinhaMaquina {
  chave: string;
  nome: string;
  identificador: string | null;
  navegador: Server | null;
  peer: PeerView | null;
  estaMaquina: boolean;
}

export function unirMaquinas(
  servidores: Server[],
  ids: Record<string, string | null>,
  peers: PeerView[],
  escolhidoId: string | null,
): LinhaMaquina[] {
  const porId = new Map(peers.map((p) => [p.id, p]));
  const usados = new Set<string>();
  // Dois servidores do navegador (LAN + Tailscale da mesma máquina) podem ter o MESMO
  // identificador: o segundo a casar fica sem peer, senão duas linhas renderizam o mesmo `corrige`.
  const idsCasados = new Set<string>();
  const linhas: LinhaMaquina[] = servidores.map((s) => {
    const identificador = ids[s.id] ?? null;
    const peer = identificador && !idsCasados.has(identificador) ? porId.get(identificador) ?? null : null;
    if (peer) { usados.add(peer.id); idsCasados.add(identificador!); }
    return {
      chave: `srv:${s.id}`,
      nome: s.label,
      identificador,
      navegador: s,
      peer,
      estaMaquina: s.id === escolhidoId,
    };
  });
  for (const p of peers) {
    if (usados.has(p.id)) continue;
    linhas.push({ chave: `peer:${p.id}`, nome: p.id, identificador: p.id, navegador: null, peer: p, estaMaquina: false });
  }
  return linhas.sort((a, b) => Number(b.estaMaquina) - Number(a.estaMaquina) || a.nome.localeCompare(b.nome));
}
