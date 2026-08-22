// Shim para '@/text' — o Happy chama t('chave', vars)
// Aqui mapeamos para messages compiladas do Hangar (prefixo happy_)
import * as m from '../../../paraglide/messages.js';

export function t(key: string, vars?: Record<string, unknown>): string {
  const fnKey = 'happy_' + key.replace(/\./g, '_');
  const fn = (m as unknown as Record<string, unknown>)[fnKey];
  if (typeof fn === 'function') {
    try {
      return (fn as (v?: unknown) => string)(vars as unknown);
    } catch {
      return key;
    }
  }
  if (typeof fn === 'string') return fn;
  return key;
}
