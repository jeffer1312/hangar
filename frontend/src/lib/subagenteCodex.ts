// Lê a notificação de subagente que o Codex grava no rollout como mensagem de USER:
//
//   <subagent_notification>
//   {"agent_path":"01a0…","status":{"completed":"relatório em markdown…"}}
//   </subagent_notification>
//
// Sem esta leitura a bolha sai com o envelope, o JSON cru e os `\n` escapados no meio do texto.
//
// Mesmas duas regras dos outros cartões (orqRecado, bastaoRecado): o que não casar volta `null` e
// cai na bolha de sempre, e o texto cru nunca some — quem o mostra é o componente, num bloco
// fechado.
import * as m from '../paraglide/messages';

export type SubagenteCodex = {
  /** Chave de `status` como o Codex a escreveu: `completed`, `errored`, ou outra que ele criar. */
  status: string;
  /** Id do subagente (`agent_path`), vazio quando ausente. */
  agentPath: string;
  /** Corpo do status — markdown, no caso do relatório. */
  texto: string;
};

const ENVELOPE = /^<subagent_notification>\s*([\s\S]*?)\s*<\/subagent_notification>$/;

/** Falhou? `errored` e `failed` são o mesmo desfecho pra quem lê; o Codex usa os dois. */
export function subagenteFalhou(status: string): boolean {
  return status === 'errored' || status === 'failed';
}

/**
 * Rótulo do status, um só pros dois lugares que o mostram (cartão do chat e linha do quadro).
 * Estava duplicado como ternário no card do quadro — a terceira cópia é que começaria a divergir.
 */
export function rotuloSubagente(status: string): string {
  if (status === 'completed') return m.subagente_card_concluido();
  if (subagenteFalhou(status)) return m.subagente_card_falhou();
  return m.subagente_card_status({ s: status });
}

export function lerSubagenteCodex(texto: string): SubagenteCodex | null {
  const casou = texto.trim().match(ENVELOPE);
  if (!casou) return null;
  let obj: unknown;
  try {
    obj = JSON.parse(casou[1]);
  } catch {
    return null;
  }
  if (!obj || typeof obj !== 'object') return null;
  const status = (obj as Record<string, unknown>).status;
  if (!status || typeof status !== 'object') return null;
  const [chave, valor] = Object.entries(status as Record<string, unknown>)[0] ?? [];
  if (!chave) return null;
  const agentPath = (obj as Record<string, unknown>).agent_path;
  return {
    status: chave,
    agentPath: typeof agentPath === 'string' ? agentPath : '',
    // Status com corpo que não é texto (o Codex pode passar a mandar objeto) ainda vira cartão:
    // o JSON legível é melhor que a bolha com o envelope inteiro.
    texto: typeof valor === 'string' ? valor : JSON.stringify(valor ?? '', null, 2),
  };
}
