import { getConfig, getConfigForServer, patchConfig, patchConfigForServer } from '@hangar/core';
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

/**
 * Soma a origem DESTE app às que o servidor aceita no terminal, e devolve o valor gravado.
 *
 * Existe porque a saída para a recusa era ler um aviso, achar Configurações → Máquinas e digitar a
 * URL num teclado de celular — no aparelho que acabou de perder o terminal.
 *
 * SOMA, nunca sobrescreve: `term_origins` é lista, e apagar a origem que alguém declarou antes
 * tiraria o outro aparelho do ar. Só mexe no que a TELA gravou: quando o valor vem do
 * `CP_TERM_ORIGINS` do `.env` (`origem: 'env'`), reenviá-lo aqui viraria uma CÓPIA no override que
 * sobreviveria à remoção da chave — e não é preciso, porque o backend soma as duas fontes
 * (`termsock.origens_extras`).
 */
export async function liberarOrigemDesteApp(srv: Server | null): Promise<string> {
  const origem = window.location.origin;
  const c = srv ? await getConfigForServer(srv) : await getConfig();
  const campo = c?.campos?.term_origins;
  const gravadas = campo?.origem === 'app' ? String(campo.valor ?? '') : '';
  const lista = gravadas.split(',').map((s) => s.trim()).filter(Boolean);
  if (!lista.includes(origem)) lista.push(origem);
  const valor = lista.join(',');
  if (srv) await patchConfigForServer(srv, { term_origins: valor });
  else await patchConfig({ term_origins: valor });
  return valor;
}
