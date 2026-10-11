// Estado da voz ao vivo como o nativo mostra: quem fala pelos níveis, senão o que a voz está fazendo.
export type Speaker = 'you' | 'voice' | 'idle';
export type VoiceStatus = 'connecting' | 'voice' | 'muted' | 'you' | 'thinking' | 'searching' | 'working' | 'listening';
type Activity = 'idle' | 'thinking' | 'searching' | 'working';

const SPEAKING = 0.08;
/** Folga antes de trocar quem fala: a fala tem pausas curtas e o rótulo não pode piscar. */
export const SPEAKER_HOLD_MS = 300;

export function speaker(input: number, output: number, muted: boolean): Speaker {
  const heard = muted ? 0 : input;
  if (heard > SPEAKING && heard >= output) return 'you';
  if (output > SPEAKING && output > heard) return 'voice';
  return 'idle';
}

/** `since`: última vez em que a leitura crua concordou com `shown`; a troca só vale depois de durar `SPEAKER_HOLD_MS`. */
export function settledSpeaker(shown: Speaker, since: number, raw: Speaker, now: number): [Speaker, number] {
  if (raw === shown) return [shown, now];
  return now - since >= SPEAKER_HOLD_MS ? [raw, now] : [shown, since];
}

export function voiceStatus(live: boolean, who: Speaker, muted: boolean, activity: Activity | undefined): VoiceStatus {
  if (!live) return 'connecting';
  if (who === 'voice') return 'voice';
  if (muted) return 'muted';
  if (who === 'you') return 'you';
  return activity && activity !== 'idle' ? activity : 'listening';
}

/** As últimas linhas do raciocínio, sem a marcação de negrito que o modelo usa nos títulos. */
export function thoughtTail(thought: string, lines = 3): string[] {
  const all = thought.split('\n').map(l => l.trim().replace(/^\*+|\*+$/g, '').trim()).filter(Boolean);
  return all.slice(-lines);
}
