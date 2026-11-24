<script lang="ts">
  // Divisor da troca de agente na conversa: a partir daqui respondeu outro modelo.
  import * as m from '../paraglide/messages';
  import { modelDisplay, type HandoffDivider } from '@hangar/core';

  let { handoff }: { handoff: HandoffDivider | null | undefined } = $props();
  const from = $derived(modelDisplay(handoff?.from_provider, handoff?.from_model));
  const to = $derived(modelDisplay(handoff?.to_provider, handoff?.to_model));
</script>

<div class="handoff" role="separator" aria-label={`${m.handoff_divider()}: ${from} → ${to}`}>
  <span class="rule" aria-hidden="true"></span>
  <span class="title">{m.handoff_divider()}</span>
  <span class="model">{from}</span>
  <span class="arrow" aria-hidden="true">→</span>
  <span class="model">{to}</span>
  <span class="rule" aria-hidden="true"></span>
</div>

<style>
  .handoff {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
  .rule { flex: 1; height: 1px; background: var(--border-default); }
  .title { font-weight: 600; }
  .arrow { color: var(--text-faint, var(--text-muted)); }
</style>
