<script lang="ts">
  import { enablePush, pushSupported } from '../lib/push';
  import { getPushSettings, getPushSettingsForServer, setQuietHours, setQuietHoursForServer } from '../lib/api';
  import type { Server } from '../lib/auth';

  // Ativação de push + horas silenciosas, com alvo explícito: global (desktop), servidor específico
  // (drawer mobile) ou indisponível (sem servidor resolvido — campos desabilitados). Extraído do
  // AccountMenu (Task 4a) pra a tela Servidores das Configurações reusar (Task 4b).
  type PushTarget =
    | { mode: 'global' }
    | { mode: 'server'; server: Server }
    | { mode: 'unavailable' };
  interface Props { target: PushTarget }
  let { target }: Props = $props();

  // Web push: liga notificação de "sessão aguardando" (assina + registra nos servidores). Reusa o
  // MESMO enablePush das duas telas — não reimplementa. O enablePush é GLOBAL, por isso o rótulo
  // diz "em todos os servidores" mesmo no drawer.
  let pushBusy = $state(false);
  let pushMsg = $state('');
  async function handleEnablePush() {
    pushBusy = true;
    pushMsg = '';
    try {
      const n = await enablePush();
      pushMsg = `Ativado em ${n} servidor${n > 1 ? 'es' : ''}.`;
    } catch (e) {
      pushMsg = e instanceof Error ? e.message : 'Erro ao ativar.';
    } finally {
      pushBusy = false;
    }
  }

  // Horas silenciosas (feature #5): janela GLOBAL de silêncio pro push, do servidor ativo. <input
  // type="time"> nativo. Carrega ao abrir o menu; best-effort (offline/sem rota -> campos vazios).
  let qhStart = $state('');
  let qhEnd = $state('');
  let qhMsg = $state('');
  // Descartar resposta de alvo ANTIGO: trocar o servidor do drawer no meio do load pintaria a janela
  // da máquina anterior como se fosse a desta. Cada load ganha um número; só o último aplica.
  let loadGeneration = 0;
  async function loadQuietHours() {
    const mine = ++loadGeneration;
    if (target.mode === 'unavailable') {
      qhStart = '';
      qhEnd = '';
      qhMsg = 'Servidor indisponível';
      return;
    }
    qhMsg = '';
    const result = target.mode === 'server'
      ? await getPushSettingsForServer(target.server)
      : await getPushSettings();
    if (mine !== loadGeneration) return;
    qhStart = result.quiet_hours?.start ?? '';
    qhEnd = result.quiet_hours?.end ?? '';
  }
  async function saveQuietHours() {
    try {
      if (target.mode === 'server') {
        await setQuietHoursForServer(target.server, qhStart || null, qhEnd || null);
      } else if (target.mode === 'global') {
        await setQuietHours(qhStart || null, qhEnd || null);
      } else {
        throw new Error('Servidor indisponível');
      }
      qhMsg = qhStart && qhEnd ? `silenciado ${qhStart}–${qhEnd}` : 'desligado';
    } catch (e) {
      qhMsg = e instanceof Error ? e.message : 'erro ao salvar';
    }
  }
  // Recarrega a janela de silêncio a cada mudança de alvo (pode ter mudado no servidor).
  $effect(() => {
    const key = target.mode === 'server' ? target.server.id : target.mode;
    void key;
    if (pushSupported()) void loadQuietHours();
  });
</script>

<button class="pq-item" role="menuitem" onclick={handleEnablePush} disabled={pushBusy}>
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>
  {pushBusy ? 'Ativando…' : 'Ativar notificações em todos os servidores'}
</button>
{#if pushMsg}<div class="pq-msg">{pushMsg}</div>{/if}
<div class="pq-quiet">
  <div class="pq-quiet-head">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z"/></svg>
    <span>Horas silenciosas</span>
  </div>
  <div class="pq-quiet-row">
    <input type="time" bind:value={qhStart} aria-label="Início do silêncio" disabled={target.mode === 'unavailable'} />
    <span>e</span>
    <input type="time" bind:value={qhEnd} aria-label="Fim do silêncio" disabled={target.mode === 'unavailable'} />
    <button class="pq-quiet-save" onclick={saveQuietHours} disabled={target.mode === 'unavailable'}>Salvar</button>
  </div>
  {#if qhMsg}<div class="pq-msg">{qhMsg}</div>{/if}
</div>

<style>
  .pq-item {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; min-height: 44px; padding: var(--space-2) var(--space-4);
    text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: 0;
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .pq-item svg { flex-shrink: 0; color: var(--text-secondary); }
  .pq-item:hover { background: var(--bg-hover); }
  .pq-item:active { background: var(--bg-hover); }
  .pq-item:disabled { color: var(--text-muted); }

  .pq-msg { font-size: var(--text-xs); color: var(--text-muted); padding: 2px var(--space-4) var(--space-1); }

  /* Horas silenciosas: cabeçalho (ícone + rótulo) + par de <input type="time"> + Salvar. */
  .pq-quiet { padding: var(--space-1) var(--space-4) var(--space-2); }
  .pq-quiet-head { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); padding: var(--space-1) 0; }
  .pq-quiet-head svg { flex-shrink: 0; color: var(--text-secondary); }
  .pq-quiet-row { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); }
  .pq-quiet-row input[type='time'] {
    min-width: 0; flex: 1;
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    color: var(--text-primary); font-size: var(--text-sm); padding: 4px 6px;
  }
  .pq-quiet-row input[type='time']:disabled { opacity: 0.5; }
  .pq-quiet-save {
    flex-shrink: 0; min-height: 0; font-size: var(--text-xs); font-weight: 600; color: var(--accent);
    padding: 5px 10px; border-radius: var(--radius-full); border: 1px solid var(--accent);
  }
  .pq-quiet-save:hover:not(:disabled) { background: var(--accent); color: #fff; }
  .pq-quiet-save:disabled { opacity: 0.5; }
</style>
