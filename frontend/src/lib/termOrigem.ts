import { getConfig, getConfigForServer } from '@hangar/core';
import type { Server } from './auth';
import * as m from '../paraglide/messages';

/**
 * O terminal caiu sem motivo: foi a ORIGEM que o servidor recusou?
 *
 * O handshake do WebSocket é recusado com 403 antes do accept, e o navegador não entrega corpo nem
 * código — a tela dizia só "desconectado", e a explicação existia apenas no log do servidor. Aqui a
 * mesma pergunta vai por HTTP, que tem resposta legível.
 *
 * Só é chamada quando o fechamento não trouxe motivo: um close com texto (o backend fecha dizendo
 * "outra conexão assumiu") já explica melhor do que este palpite.
 */
export async function motivoDeOrigemRecusada(srv: Server | null): Promise<string | null> {
  try {
    const c = srv ? await getConfigForServer(srv) : await getConfig();
    if ((c?.somente_leitura as Record<string, unknown> | undefined)?.terminal_origem_ok === false) {
      return m.term_origem_recusada({ origem: window.location.origin });
    }
  } catch {
    // Servidor fora do ar responde a mesma coisa que uma origem recusada — nada. Cair no motivo
    // genérico é o certo: afirmar "origem recusada" aqui inventaria uma causa.
  }
  return null;
}
