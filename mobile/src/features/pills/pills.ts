import type { StatusFields } from '@hangar/core';

// rótulo que a pílula mostra: escolhido otimista prevalece sobre o que veio da statusline
export function pillLabels(
  f: StatusFields | null,
  chosen: { model?: string | null; effort?: string | null },
): { model: string | null; effort: string | null } {
  return {
    model: chosen.model ?? f?.model ?? null,
    effort: chosen.effort ?? f?.effort ?? null,
  };
}

// quando a statusline confirma o que foi escolhido (substring), solta o otimista
// porte de Composer.svelte:571-577 — só o modelo tem read-back confiável, esforço é write-only
export function reconcileChosen(
  f: StatusFields | null,
  chosen: { model?: string | null; effort?: string | null },
): { model?: string | null; effort?: string | null } {
  if (!f?.model || !chosen.model) return chosen;
  if (f.model.toLowerCase().includes(chosen.model.toLowerCase())) {
    return { ...chosen, model: null };
  }
  return chosen;
}

// haiku não usa esforço (picker responde "Effort not supported")
export function semEsforco(model: string | null | undefined): boolean {
  return !!model && model.toLowerCase().includes('haiku');
}
