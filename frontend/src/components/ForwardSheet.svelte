<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getSessions, sendInput } from '../lib/api';
  import { stateLabels, stateColors } from '../lib/format';
  import type { SessionInfo } from '../lib/types';

  interface Props {
    open: boolean;
    text: string;         // conteudo da bolha a encaminhar
    fromSession: string;  // sessao atual (vira o "[de: X]" no destino)
    onClose: () => void;
  }
  let { open, text, fromSession, onClose }: Props = $props();

  let sessions = $state<SessionInfo[]>([]);
  // idle: escolhendo | nome da sessao: enviando/enviado (feedback por linha)
  let sentTo = $state<string | null>(null);
  let error = $state<string | null>(null);

  // Recarrega a lista a cada abertura (sessao pode ter nascido/morrido desde a ultima).
  // epoch: o BottomSheet mantem o componente MONTADO entre aberturas — abrir/fechar/reabrir rapido
  // deixava a resposta da abertura ANTIGA resolver depois e sobrescrever a nova (dado stale).
  let epoch = 0;
  $effect(() => {
    if (!open) return;
    const my = ++epoch;
    sentTo = null;
    error = null;
    getSessions()
      .then((all) => { if (my === epoch) sessions = all.filter((s) => s.name !== fromSession && s.state !== 'dead'); })
      .catch(() => { if (my === epoch) error = 'Não deu pra listar as sessões.'; });
  });

  async function forward(target: string) {
    if (sentTo) return; // ja enviou (ou enviando) -> ignora toque duplo
    sentTo = target;
    try {
      // Mesmo formato do cp-send: destino mostra o chip "de: <sessao>" (bolha peer).
      await sendInput(target, `[de: ${fromSession}] ${text}`);
      setTimeout(onClose, 700); // deixa o ✓ visivel um instante
    } catch {
      sentTo = null;
      error = `Falhou o envio pra ${target}.`;
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Encaminhar pra outra sessão">
  <div class="fwd">
    <h2 class="title">Encaminhar pra sessão</h2>
    <p class="excerpt">{text.length > 160 ? text.slice(0, 160) + '…' : text}</p>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    <div class="list">
      {#if sessions.length === 0 && !error}
        <p class="empty">Nenhuma outra sessão viva.</p>
      {:else}
        {#each sessions as s (s.name)}
          <button class="row" onclick={() => forward(s.name)} disabled={sentTo != null}
                  aria-label={`Encaminhar pra ${s.name} — ${stateLabels[s.state]}`}>
            <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
            <span class="row-main">
              <span class="row-name">{s.name}</span>
              {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
            </span>
            {#if sentTo === s.name}<span class="sent">✓ enviado</span>{/if}
          </button>
        {/each}
      {/if}
    </div>
  </div>
</BottomSheet>

<style>
  .fwd { padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3); }

  .title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }

  /* Trecho a encaminhar: citacao curta, so contexto visual. */
  .excerpt {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    border-left: 3px solid var(--accent);
    padding-left: var(--space-3);
    word-break: break-word;
    max-height: 5.5em;
    overflow: hidden;
  }

  .error { font-size: var(--text-sm); color: #e5484d; }
  .empty { font-size: var(--text-sm); color: var(--text-muted); padding: var(--space-3) 0; }

  .list { display: flex; flex-direction: column; }

  .row {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; padding: var(--space-3);
    background: none; border: none; border-radius: var(--radius-md);
    text-align: left; cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .row:hover { background: var(--bg-hover); }
  .row:disabled { cursor: default; opacity: 0.7; }

  .dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }

  .row-main { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .row-name { font-size: var(--text-base); color: var(--text-primary); font-weight: 500; }
  .row-cwd {
    font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }

  .sent { font-size: var(--text-sm); color: var(--accent); font-weight: 600; flex-shrink: 0; }
</style>
