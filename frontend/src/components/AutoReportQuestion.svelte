<script lang="ts">
  // Pergunta única do relatório automático da atualização: aparece na abertura enquanto alguma máquina
  // própria não respondeu, e uma resposta vale para todas elas. Sem resposta, nada sai da máquina.
  import { onMount } from 'svelte';
  import { autoReportUnanswered, putAutoReportEm, type Server } from '@hangar/core';
  import { listOwnServers } from '../lib/auth';
  import BottomSheet from './BottomSheet.svelte';
  import * as m from '../paraglide/messages';

  let pending = $state<Server[]>([]);
  let saving = $state(false);
  let error = $state('');

  onMount(() => {
    autoReportUnanswered(listOwnServers()).then((servers) => { pending = servers; });
  });

  async function answer(value: boolean) {
    saving = true; error = '';
    const results = await Promise.allSettled(pending.map((server) => putAutoReportEm(server, value)));
    const failed = pending.filter((_, i) => results[i].status === 'rejected');
    saving = false;
    if (failed.length) {
      error = failed.map((s) => s.label).join(', ');
      pending = failed;
      return;
    }
    pending = [];
  }
</script>

<BottomSheet open={pending.length > 0} onClose={() => (pending = [])} ariaLabel={m.auto_report_question_title()}>
  <div class="pergunta">
    <h2>{m.auto_report_question_title()}</h2>
    <p>{m.auto_report_question_body({ machines: pending.map((s) => s.label).join(', ') })}</p>
    {#if error}<p class="erro" role="alert">{m.auto_report_save_failed({ machines: error })}</p>{/if}
    <div class="acoes">
      <button class="bt secundario" disabled={saving} onclick={() => answer(false)}>{m.auto_report_no()}</button>
      <button class="bt primario" disabled={saving} onclick={() => answer(true)}>{m.auto_report_yes()}</button>
    </div>
  </div>
</BottomSheet>

<style>
  .pergunta { display: flex; flex-direction: column; gap: 12px; padding: 16px; }
  h2 { margin: 0; font-size: var(--text-lg); }
  p { margin: 0; }
  .erro { color: var(--error); }
  .acoes { display: flex; justify-content: flex-end; gap: var(--space-2); }
  .bt { border-radius: var(--radius-md); padding: var(--space-2) var(--space-4);
        font-family: inherit; font-size: var(--text-sm); font-weight: 600; border: 0; min-height: 44px; }
  .primario { background: var(--accent); color: var(--text-inverse); }
  .secundario { background: var(--surface-raised); color: var(--text-secondary);
                border: 1px solid var(--border-subtle); font-weight: 500; }
  .bt:disabled { opacity: 0.5; }
</style>
