<script lang="ts">
  import type { Snippet } from 'svelte';
  import IconSend from '../icons/IconSend.svelte';
  import * as m from '../../paraglide/messages';

  // Compositor da conversa que ainda vai nascer (tela inicial e leitura de conversa fechada):
  // campo que cresce, pílulas à esquerda dentro dele, enviar à direita; `below` fica entre ele e o aviso.
  interface Props {
    value: string;
    placeholder: string;
    /** Envio em voo: campo só leitura, enviar travado. */
    busy?: boolean;
    /** Só o botão enviar travado (falta pasta, conta sem limite…). */
    blocked?: boolean;
    note?: { text: string; warning: boolean } | null;
    onsend: () => void;
    pills?: Snippet;
    below?: Snippet;
    /** Botões à esquerda das pílulas ("+" e microfone) e, acima do campo, os anexos escolhidos. */
    leading?: Snippet;
    top?: Snippet;
    /** Anexo sozinho já é mensagem: o envio não exige texto. */
    hasAttachments?: boolean;
  }
  let { value = $bindable(''), placeholder, busy = false, blocked = false, note = null, onsend, pills, below, leading, top,
    hasAttachments = false }: Props = $props();

  let field: HTMLTextAreaElement | undefined = $state();
  const canSend = $derived(!busy && !blocked && (value.trim().length > 0 || hasAttachments));

  // Cresce com o texto até um teto; acima disso o campo rola.
  $effect(() => {
    void value;
    if (!field) return;
    field.style.height = 'auto';
    field.style.height = `${Math.min(field.scrollHeight, 200)}px`;
  });

  function send() {
    if (canSend) onsend();
  }

  // Mesma regra do Composer da conversa: Enter envia só no layout largo (teclado físico); no
  // celular Enter quebra linha e o envio é pelo botão.
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && window.matchMedia('(min-width: 820px)').matches) {
      e.preventDefault();
      send();
    }
  }
</script>

<div class="wrap">
  <div class="box" class:busy>
    <!-- readonly, não disabled: desabilitar tira o foco e fecha o teclado do iPhone no meio do envio. -->
    {@render top?.()}
    <textarea bind:this={field} bind:value rows="1" {placeholder} aria-label={placeholder} readonly={busy} aria-busy={busy} {onkeydown}></textarea>
    <div class="bar">
      {#if leading}<div class="lead">{@render leading()}</div>{/if}
      <div class="pills">{@render pills?.()}</div>
      <button type="button" class="send" disabled={!canSend} aria-label={m.native_send()} onclick={send}>
        <IconSend size={18} />
      </button>
    </div>
  </div>
  {@render below?.()}
  {#if note}
    <p class="note" class:warning={note.warning} role={note.warning ? 'alert' : 'status'}>{note.text}</p>
  {/if}
</div>

<style>
  .wrap { display: flex; flex-direction: column; gap: 2px; }
  .box {
    display: flex; flex-direction: column; gap: 4px; padding: 10px 10px 8px 14px;
    border: 1px solid var(--border-default); border-radius: var(--radius-xl); background: var(--surface-raised);
  }
  .box.busy { opacity: 0.7; }
  textarea {
    width: 100%; min-height: 24px; max-height: 200px; resize: none; border: 0; outline: none; padding: 4px 0;
    background: transparent; color: var(--text-primary); font: inherit; font-size: var(--text-base); line-height: 1.4;
  }
  textarea::placeholder { color: var(--text-muted); }
  .bar { display: flex; align-items: center; gap: 8px; }
  .pills { flex: 1; min-width: 0; display: flex; align-items: center; gap: 2px; margin-left: -10px; }
  .lead { display: flex; align-items: center; margin-left: -10px; }
  .lead + .pills { margin-left: 0; }
  .send {
    flex: none; width: 34px; height: 34px; display: inline-flex; align-items: center; justify-content: center;
    border: 0; border-radius: var(--radius-full); background: var(--accent); color: #fff; cursor: pointer;
  }
  .send:disabled { background: var(--bg-hover); color: var(--text-muted); cursor: default; }
  .note { margin: 4px 0 0; padding: 0 14px; font-size: var(--text-sm); color: var(--text-muted); }
  .note.warning { color: var(--warning-text); }
</style>
