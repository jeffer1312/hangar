<script lang="ts">
  import { enablePush, pushSupported } from '../lib/push';
  import { getPushSettings, getPushSettingsForServer, setQuietHours, setQuietHoursForServer } from '../lib/api';
  import type { Server } from '../lib/auth';

  // Ativação de push + horas silenciosas, com alvo explícito: global (desktop), servidor específico
  // (drawer mobile) ou indisponível (sem servidor resolvido — campos desabilitados). Extraído do
  // AccountMenu (Task 4a) pra a tela Servidores das Configurações reusar (Task 4b).
  //
  // `open` NÃO controla visibilidade: o pai mantém este componente SEMPRE montado (fechar/reabrir o
  // menu não pode perder busy/resultado de push nem a janela de silêncio carregada) — ele só decide
  // QUANDO carregar, preservando o lifecycle antigo (nada de load no arranque do app).
  type PushTarget =
    | { mode: 'global' }
    | { mode: 'server'; server: Server }
    | { mode: 'unavailable' };
  interface Props { target: PushTarget; open: boolean }
  let { target, open }: Props = $props();

  // Web push: liga notificação de "sessão aguardando" (assina + registra nos servidores). Reusa o
  // MESMO enablePush das duas telas — não reimplementa. O enablePush é GLOBAL, por isso o rótulo
  // diz "em todos os servidores" mesmo no drawer.
  let pushBusy = $state(false);
  let pushMsg = $state('');
  async function handleEnablePush() {
    if (pushBusy) return;
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
  let loading = $state(false);
  let saving = $state(false);
  // Geração da operação em voo: descarta resposta de load de um alvo/abertura ANTERIOR e impede um
  // load velho repintar os campos depois de um save. Toda operação nova incrementa; quem resolve
  // com o número velho não pinta nada.
  let loadGeneration = 0;
  async function loadQuietHours() {
    const mine = ++loadGeneration;
    const alvo = target;   // snapshot: este load responde SOBRE o alvo que o disparou
    if (alvo.mode === 'unavailable') {
      qhStart = '';
      qhEnd = '';
      qhMsg = 'Servidor indisponível';
      return;
    }
    loading = true;
    qhMsg = '';
    try {
      const result = alvo.mode === 'server'
        ? await getPushSettingsForServer(alvo.server)
        : await getPushSettings();
      if (mine !== loadGeneration) return;   // alvo mudou ou houve save no meio: não repinta
      qhStart = result.quiet_hours?.start ?? '';
      qhEnd = result.quiet_hours?.end ?? '';
    } catch (e) {
      if (mine !== loadGeneration) return;
      // Global segue best-effort (offline/rota ausente -> campos vazios, salvar depois resolve).
      // Por-servidor NÃO: `apiFetchForServer` não faz o self-heal de 401 de propósito (não pode
      // derrubar a credencial de outra máquina), então token morto ficaria como "campos vazios"
      // pra sempre — indistinguível de "nunca configurei" — e ninguém iria trocar o token.
      if (alvo.mode === 'server') qhMsg = e instanceof Error ? e.message : 'não foi possível carregar';
    } finally {
      if (mine === loadGeneration) loading = false;
    }
  }
  async function saveQuietHours() {
    if (saving || loading) return;   // duplo clique, ou save durante load: ignora
    const alvo = target;             // snapshot: salva o que o usuário editou NESTE alvo
    const inicio = qhStart;
    const fim = qhEnd;
    saving = true;
    const mine = ++loadGeneration;   // invalida loads em voo: nada repinta depois do save
    qhMsg = '';
    try {
      if (alvo.mode === 'server') {
        await setQuietHoursForServer(alvo.server, inicio || null, fim || null);
      } else if (alvo.mode === 'global') {
        await setQuietHours(inicio || null, fim || null);
      } else {
        throw new Error('Servidor indisponível');
      }
      if (mine !== loadGeneration) return;   // outro load/save tomou a frente: não sobrescreve msg
      qhMsg = inicio && fim ? `silenciado ${inicio}–${fim}` : 'desligado';
    } catch (e) {
      if (mine !== loadGeneration) return;
      qhMsg = e instanceof Error ? e.message : 'erro ao salvar';
    } finally {
      saving = false;   // flag de entrada: se outro save começou, o guard dele já bloqueia; a msg
                        // pintada acima é a do dono da geração atual
    }
  }
  // Recarrega a janela de silêncio a cada mudança de alvo OU abertura do menu (pode ter mudado no
  // servidor). Chave por PRIMITIVO (id/modo), não pelo objeto: `target` é re-criado a cada render
  // do pai e um efeito que o lesse recarregaria a cada recomputo.
  const alvoKey = $derived(target.mode === 'server' ? target.server.id : target.mode);
  $effect(() => {
    void alvoKey;
    if (!open || !pushSupported()) return;
    // Alvo trocou: limpa o que o alvo anterior mostrou antes de carregar o novo.
    qhStart = '';
    qhEnd = '';
    qhMsg = '';
    void loadQuietHours();
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
    <input type="time" bind:value={qhStart} aria-label="Início do silêncio" disabled={target.mode === 'unavailable' || loading || saving} />
    <span>e</span>
    <input type="time" bind:value={qhEnd} aria-label="Fim do silêncio" disabled={target.mode === 'unavailable' || loading || saving} />
    <button class="pq-quiet-save" onclick={saveQuietHours} disabled={target.mode === 'unavailable' || loading || saving}>Salvar</button>
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

  button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
</style>
