import { integrationFailureText } from '@hangar/core';

/** Texto da falha das integrações: a frase do core, senão a mensagem crua do pedido. */
export function integrationErrorText(e: unknown): string {
  return integrationFailureText(e) ?? (e instanceof Error ? e.message : String(e));
}
