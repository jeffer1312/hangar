// Troca de agente (context handoff): o nome curto de cada modelo no divisor da conversa e o título da faixa da
// troca pendente. O nome segue a regra do `model_display` do nativo (desktop-native/src/app/controls/picker.rs).
import * as m from './paraglide/messages';
import { mensagemDeErro } from './errosApi';
import { providerName } from './format';
import { rotuloModelo } from './statusline';
import type { SessionHandoff, SessionInfo } from './types';

const capitalized = (word: string): string => (word ? word[0]!.toUpperCase() + word.slice(1) : word);

/** `claude-opus-5-5` vira "Opus 5.5" e `gpt-6.2-sol` vira "GPT-6.2-Sol"; sem modelo, o nome do agente. */
export function modelDisplay(provider: string | null | undefined, model: string | null | undefined): string {
  const id = (model ?? '').trim();
  if (!id) return providerName(provider as SessionInfo['provider']);
  if (provider === 'claude') return rotuloModelo(id);
  const wide = id.endsWith('[1m]');
  const shown = (wide ? id.slice(0, -4) : id).split('-').map((p) => (p.toLowerCase() === 'gpt' ? 'GPT' : capitalized(p))).join('-');
  return wide ? `${shown} 1M` : shown;
}

/** O título da faixa: trocando, falhou (com o motivo traduzido), ou esperando o turno da sessão que trabalha. */
export function handoffTitle(handoff: SessionHandoff, sessionState: string | null | undefined): string {
  if (handoff.state === 'running') return m.handoff_running();
  if (handoff.state === 'failed') {
    const code = handoff.code ?? '';
    return m.handoff_failed({ reason: (code && mensagemDeErro(code)) || code });
  }
  return sessionState === 'working' ? m.handoff_waiting_turn() : m.handoff_waiting();
}
