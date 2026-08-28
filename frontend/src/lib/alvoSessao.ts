// Para onde o botão "Abrir <nome>" do cartão de recado do hangar deve levar — ou se ele deve
// existir.
//
// O cartão tirava o alvo do comando (`hangar-send <alvo> "..."`) ou do campo `to` do `SendMessage`
// e montava `#/chat/<servidor-da-rota-atual>/<alvo>` direto. Esse alvo é um ENDEREÇO DE RECADO, e
// endereço de recado não é nome de sessão do app. Dois jeitos de furar, os dois vistos em uso:
//
//   1. `servidor-b::thread-admin` — sessão de OUTRA máquina. Virava uma rota com o `::` inteiro no
//      lugar do nome, no servidor errado. Clique morto.
//   2. um nome de subagente — o `to` do `SendMessage` também aceita subagente e outros destinos
//      que não têm chat nenhum no app. Clique morto do mesmo jeito.
//
// Aqui o alvo só vira rota quando existe MESMO uma sessão com aquele nome naquele servidor. Sem
// isso, `null`: quem chama não desenha o botão. Botão que não faz nada é pior que botão ausente —
// ele promete uma tela que não existe.
import type { ServerBucket } from './sessions';

export interface RotaSessao {
  serverId: string;
  nome: string;
}

/** O `servidor` de um endereço `servidor::sessao` casa com o id, com o rótulo ou com o host. São
 *  três nomes pra mesma máquina: o id do peer vem do `peers.json` do BACKEND, e o id do servidor no
 *  app vem do cadastro no aparelho — não há razão pra serem iguais. */
function achaBucket(servidor: string, buckets: readonly ServerBucket[]): ServerBucket | null {
  const alvo = servidor.toLowerCase();
  for (const b of buckets) {
    if (b.server.id.toLowerCase() === alvo || (b.server.label ?? '').toLowerCase() === alvo) return b;
    try {
      if (new URL(b.server.baseUrl).hostname.toLowerCase().split('.')[0] === alvo) return b;
    } catch {
      // baseUrl inválida no cadastro: só não casa por host, os outros dois critérios seguem.
    }
  }
  return null;
}

export function rotaDoAlvo(
  alvo: string | null | undefined,
  buckets: readonly ServerBucket[],
  servidorAtual: string | null,
): RotaSessao | null {
  if (!alvo) return null;
  const i = alvo.indexOf('::');
  const servidor = i >= 0 ? alvo.slice(0, i) : null;
  const nome = i >= 0 ? alvo.slice(i + 2) : alvo;
  if (!nome) return null;

  // Endereço de outra máquina NUNCA cai no servidor atual: abriria uma sessão homônima errada, que
  // é pior que não abrir nada. Servidor desconhecido também não vira botão — não há pra onde ir.
  const b = servidor
    ? achaBucket(servidor, buckets)
    : (servidorAtual ? buckets.find((x) => x.server.id === servidorAtual) ?? null : null);
  if (!b) return servidor || !servidorAtual ? null : { serverId: servidorAtual, nome };

  // A lista carregada é o que permite CONFERIR se a sessão existe; sem ela, conferir é impossível e
  // esconder o botão seria inventar uma resposta. Isso não é caso de borda: no celular o chat e a
  // lista de sessões são telas EXCLUDENTES (App.svelte), então ao abrir a conversa o único
  // consumidor do store desmonta, o refcount zera e os buckets ficam vazios. Exigir a lista ali
  // esconderia justo o botão de recado pra outra máquina — o que esta correção veio habilitar.
  // (achado das duas revisões, antes de ir pro repositório)
  if (!b.loaded) return { serverId: b.server.id, nome };
  return b.sessions.some((s) => s.name === nome) ? { serverId: b.server.id, nome } : null;
}
