// "Abrir navegador" que chega pelo stream da LISTA (evento 'nav' com {name, url}): é o caminho
// que funciona com a sessão FORA da tela — o chat dela não está montado, então ninguém mais
// ouviria. Marca o store (é o que faz o NavegadorPane reexibir quando o usuário abrir a sessão) e,
// no shell desktop, já cria o view escondido: o agente dirige por CDP desde agora. Fora do shell
// (celular, PWA) fica só a marca — não há view pra criar.
import { confirmarNavForServer } from './api';
import type { Server } from './auth';
import { navegadorNativo } from './navegadorNativo';
import { atualizarNavUrl, marcarNavAberto } from './navegadorPanel.svelte';
import { workspaceSessionKey } from './workspaceCommands';

export async function navPelaLista(s: Server, data: string): Promise<void> {
  let name = '';
  let url = '';
  try {
    const p = JSON.parse(data) as { name?: string; url?: string };
    name = p.name ?? '';
    url = p.url ?? '';
  } catch {
    return;
  }
  if (!name || !url) return;
  const chave = workspaceSessionKey({ serverId: s.id, name });
  marcarNavAberto(chave);
  atualizarNavUrl(chave, url);
  const nativo = navegadorNativo();
  if (!nativo) return;
  const r = await nativo.open(chave, url, { x: 0, y: 0, width: 0, height: 0 }, { oculto: true }).catch(() => ({ ok: false }));
  // Só quem criou o view confirma: no celular o marcador tem que sobreviver até o desktop ver.
  if (r.ok) await confirmarNavForServer(s, name).catch(() => {});
}
