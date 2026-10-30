import { vaultErrorText as textFor } from '@hangar/core';

/** Texto da recusa do cofre: a frase do core pelo código, senão a mensagem crua do pedido. */
export function vaultErrorText(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  return (typeof code === 'string' ? textFor(code) : undefined) ?? (e instanceof Error ? e.message : String(e));
}
