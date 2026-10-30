<script lang="ts">
  // A janela do login transmitida da máquina do navegador: mostra e leva clique e digitação.
  import { onDestroy } from 'svelte';
  import type { Server } from '../../lib/auth';
  import { entradaTela, quadroTela, reativarSessaoTela, type EntradaTela } from '../../lib/loginConta';
  import * as m from '../../paraglide/messages';

  let { alvo, conta, maquina, onFechar }: { alvo: Server | null; conta: string; maquina: string; onFechar: () => void } = $props();

  let imagem = $state<HTMLImageElement>();
  let fonte = $state('');
  let titulo = $state('');
  let aviso = $state('');
  let semEntrada = $state(false);
  let texto = $state('');
  let tamanho = { w: 1, h: 1 };
  let vivo = true;
  let ultimoMovimento = 0;
  // Fila única: clique e tecla têm que chegar na ordem em que foram feitos.
  let fila: Promise<unknown> = Promise.resolve();

  function enviar(entrada: EntradaTela) {
    fila = fila.then(() => entradaTela(alvo, conta, entrada)).catch((e: { status?: number }) => {
      if (e.status === 409) semEntrada = true;
    });
  }

  async function buscar() {
    while (vivo) {
      try {
        const quadro = await quadroTela(alvo, conta);
        if (!vivo) return;
        if (quadro.d) {
          fonte = `data:image/jpeg;base64,${quadro.d}`;
          tamanho = { w: quadro.w ?? 1, h: quadro.h ?? 1 };
          titulo = quadro.title ?? '';
          semEntrada = quadro.input === false;
          aviso = '';
        } else if (!quadro.available) aviso = m.native_accounts_login_view_unavailable();
        else if (quadro.closed) aviso = m.native_accounts_login_view_closed();
        else if (quadro.pending) aviso = m.native_accounts_login_view_waiting();
        else aviso = m.native_accounts_login_view_capture();
      } catch {
        if (vivo) aviso = m.native_accounts_login_view_lost();
      }
      await new Promise((resolver) => setTimeout(resolver, 350));
    }
  }
  void buscar();
  onDestroy(() => { vivo = false; });

  function ponto(e: MouseEvent | PointerEvent) {
    const caixa = imagem?.getBoundingClientRect();
    if (!caixa || !caixa.width || !caixa.height) return null;
    return { x: ((e.clientX - caixa.left) * tamanho.w) / caixa.width, y: ((e.clientY - caixa.top) * tamanho.h) / caixa.height };
  }

  function apertar(e: PointerEvent, action: 'down' | 'up') {
    if (e.button !== 0) return;
    e.preventDefault();
    if (action === 'down') imagem?.focus();
    const p = ponto(e);
    if (p) enviar({ t: 'mouse', action, ...p });
  }

  function mover(e: PointerEvent) {
    const agora = Date.now();
    if (agora - ultimoMovimento < 80) return;
    ultimoMovimento = agora;
    const p = ponto(e);
    if (p) enviar({ t: 'mouse', action: 'move', ...p });
  }

  function rolar(e: WheelEvent) {
    e.preventDefault();
    const p = ponto(e);
    if (p && e.deltaY !== 0) enviar({ t: 'wheel', ...p, down: e.deltaY > 0 });
  }

  const TECLAS = ['Enter', 'Tab', 'Backspace', 'Delete', 'Escape', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'];

  // Sem atalhos: só digitação e as teclas de navegar.
  function tecla(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'v') return;
    e.preventDefault();
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (TECLAS.includes(e.key)) enviar({ t: 'key', key: e.key });
    else if (e.key.length === 1) enviar({ t: 'text', text: e.key });
  }

  function colar(e: ClipboardEvent) {
    e.preventDefault();
    const colado = e.clipboardData?.getData('text');
    if (colado) enviar({ t: 'text', text: colado });
  }

  // No celular não há teclado sem um campo: o que é digitado aqui vai de uma vez.
  function mandarTexto(e: SubmitEvent) {
    e.preventDefault();
    if (texto) enviar({ t: 'text', text: texto });
    texto = '';
  }

  async function reativar() {
    try { await reativarSessaoTela(alvo, conta); } catch { aviso = m.native_accounts_login_view_lost(); }
  }
</script>

<div class="lv">
  <div class="lv-barra">
    <button type="button" class="lv-btn" onclick={() => enviar({ t: 'back' })}>{m.native_accounts_login_view_back()}</button>
    <span class="lv-titulo">{titulo}</span>
    <button type="button" class="lv-btn" onclick={onFechar}>{m.native_accounts_login_view_close()}</button>
  </div>
  {#if semEntrada}
    <div class="lv-sessao" role="status">
      <p>{m.native_accounts_login_view_no_input({ machine: maquina })} {m.native_accounts_login_view_activate_help()}</p>
      <button type="button" class="lv-btn" onclick={reativar}>{m.native_accounts_login_view_activate()}</button>
    </div>
  {/if}
  {#if aviso}<p class="lv-aviso" role="status">{aviso}</p>{/if}
  {#if fonte}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <img bind:this={imagem} class="lv-tela" src={fonte} alt={m.native_accounts_login_view_open()} tabindex="0" draggable="false"
      onpointerdown={(e) => apertar(e, 'down')} onpointerup={(e) => apertar(e, 'up')} onpointermove={mover}
      onwheel={rolar} onkeydown={tecla} onpaste={colar} oncontextmenu={(e) => e.preventDefault()} />
    <form class="lv-digitar" onsubmit={mandarTexto}>
      <input type="text" autocomplete="off" autocapitalize="none" spellcheck={false} bind:value={texto}
        aria-label={m.native_accounts_login_view_type()} placeholder={m.native_accounts_login_view_type()} />
      <button type="submit" class="lv-btn" disabled={!texto}>{m.native_accounts_login_view_send()}</button>
      <button type="button" class="lv-btn" onclick={() => enviar({ t: 'key', key: 'Enter' })}>Enter</button>
    </form>
  {/if}
</div>

<style>
  .lv { display: flex; flex-direction: column; gap: var(--space-3); min-width: 0; }
  .lv-barra, .lv-digitar, .lv-sessao { display: flex; align-items: center; gap: var(--space-2); min-width: 0; }
  .lv-titulo { flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--text-muted); font-size: 12px; }
  .lv-sessao p { flex: 1; min-width: 0; margin: 0; font-size: 12px; color: var(--text-muted); }
  .lv-btn { flex-shrink: 0; height: 36px; padding: 0 var(--space-3); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    background: var(--surface-raised); color: inherit; font: inherit; font-size: 13px; cursor: pointer; }
  .lv-btn:disabled { opacity: 0.5; cursor: default; }
  .lv-btn:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .lv-aviso { margin: 0; font-size: 12px; color: var(--text-muted); }
  .lv-tela { display: block; width: 100%; max-height: 70dvh; object-fit: contain; border-radius: var(--radius-md, 8px);
    touch-action: none; outline: none; user-select: none; }
  .lv-tela:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .lv-digitar input { flex: 1; min-width: 0; height: 36px; padding: 0 var(--space-3); box-sizing: border-box;
    border: 1px solid var(--border-default); border-radius: var(--radius-md, 8px); background: transparent; color: inherit; }
</style>
