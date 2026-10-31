import { vaultFailureText } from '@hangar/core';

/** Texto da recusa do cofre: a frase do core, senão a mensagem crua do pedido. */
export function vaultErrorText(e: unknown): string {
  return vaultFailureText(e) ?? (e instanceof Error ? e.message : String(e));
}
