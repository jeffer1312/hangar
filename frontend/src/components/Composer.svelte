<script module lang="ts">
  import type { CommandInfo } from '../lib/types';
  // Cache de comandos por sessao: sobrevive a remontagens do Composer (ex: voltar de
  // awaiting_input) pra buscar a lista so uma vez por sessao.
  const commandCache = new Map<string, CommandInfo[]>();
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import IconSend from './icons/IconSend.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';
  import ContextRing from './ContextRing.svelte';
  import ModelEffortSheet from './ModelEffortSheet.svelte';
  import SlashSuggest from './SlashSuggest.svelte';
  import CommandSheet from './CommandSheet.svelte';
  import ConfirmSheet from './ConfirmSheet.svelte';
  import { getCommands, setModelEffort, uploadImage, type ModelEffortBody } from '../lib/api';
  import type { State } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    sessionName: string;
    sessionState: State;
    status: StatusFields | null;
    onSend: (text: string) => void;
    onCommand: (cmd: string) => void;
    onInterrupt: () => void;
    onExpandUsage: () => void;
  }
  let { sessionName, sessionState, status, onSend, onCommand, onInterrupt, onExpandUsage }: Props = $props();

  // ── Slash commands: busca uma vez por sessao (com cache) ────────────────────
  // Comeca vazio; o $effect popula na hora a partir do cache (sincrono) ou da rede.
  let commands = $state<CommandInfo[]>([]);
  let commandSheetOpen = $state(false);
  let confirmStopOpen = $state(false);

  $effect(() => {
    const sn = sessionName;
    const cached = commandCache.get(sn);
    if (cached) {
      commands = cached;
      return;
    }
    getCommands(sn)
      .then((c) => {
        commandCache.set(sn, c);
        commands = c;
      })
      .catch(() => {
        // endpoint indisponivel -> segue com lista vazia, sem quebrar a UI
      });
  });

  let inputText = $state('');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  // ── Anexo de imagem: arquivo escolhido + preview local + estado de upload ───
  let attachment = $state<File | null>(null);
  let attachmentUrl = $state<string | null>(null);
  let fileInput: HTMLInputElement | undefined = $state();
  let uploading = $state(false);
  let attachError = $state('');

  const canSend = $derived((inputText.trim().length > 0 || attachment !== null) && !uploading);
  const isWorking = $derived(sessionState === 'working');

  // ── Pill de modelo + esforco: abre o ModelEffortSheet (aplica via endpoint dedicado) ──
  // Display otimista: a escolha aparece na hora; o status (read-back real do statusline)
  // reconcilia o modelo quando confirma. Esforco e write-only (sem read-back confiavel)
  // -> a escolha local persiste.
  let sheetOpen = $state(false);
  let chosenModel = $state<string | null>(null);   // rotulo otimista: 'Opus' | 'Sonnet' | ...
  let chosenEffort = $state<string | null>(null);   // low | medium | high | xhigh | max | ultracode

  const pillModel = $derived(chosenModel ?? status?.model ?? null);
  const pillEffort = $derived(chosenEffort ?? status?.effort ?? null);
  const pillText = $derived(
    pillModel ? pillModel + (pillEffort ? ' · ' + pillEffort : '') : 'Modelo'
  );

  // Reconciliacao do modelo: quando o statusline confirma a escolha (substring match),
  // solta a escolha otimista pra que mudancas feitas direto no terminal reaparecam.
  $effect(() => {
    const m = status?.model?.toLowerCase();
    if (chosenModel && m && m.includes(chosenModel.toLowerCase())) {
      chosenModel = null;
    }
  });

  // Aplica modelo+esforco via o endpoint dedicado, que dirige o picker interativo do /model
  // (scope 'session' = so esta sessao, sem virar o default). Devolve a Promise pro sheet
  // aguardar/tratar erro. O display otimista so muda APOS sucesso (uma aplicacao que falha
  // nao deixa o pill mostrando uma escolha que nao pegou). 'default' resolve pra um modelo
  // concreto -> deixa o statusline ditar o rotulo; os demais aparecem capitalizados.
  function handleApply(body: ModelEffortBody): Promise<void> {
    return setModelEffort(sessionName, body).then(() => {
      if (body.model) {
        chosenModel = body.model === 'default'
          ? null
          : body.model.charAt(0).toUpperCase() + body.model.slice(1);
      }
      if (body.effort) chosenEffort = body.effort;
    });
  }

  // ── Slash commands: preencher x enviar ──────────────────────────────────────
  // Preenche "/nome " no textarea e devolve o foco (pro usuario digitar o argumento).
  async function fillCommand(name: string) {
    inputText = '/' + name + ' ';
    await tick();
    autoGrow();
    textareaEl?.focus();
  }

  // Envia o comando e limpa o textarea (zero-arg ou apos confirmacao).
  function runCommand(cmd: string) {
    inputText = '';
    if (textareaEl) textareaEl.style.height = 'auto';
    onCommand(cmd);
  }

  // Toque numa sugestao do strip inline. model/effort abrem o sheet; comando com argumento
  // (ou destrutivo) preenche pra revisao antes de enviar; o resto envia direto.
  function handleSuggestPick(cmd: CommandInfo) {
    if (cmd.name === 'model' || cmd.name === 'effort') {
      inputText = '';
      sheetOpen = true;
      return;
    }
    if (cmd.argumentHint || cmd.destructive) {
      fillCommand(cmd.name);
      return;
    }
    runCommand('/' + cmd.name);
  }

  // ── Textarea: auto-grow ate 120px ──────────────────────────────────────────
  function autoGrow() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 120) + 'px';
  }

  function handleInput() {
    autoGrow();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  // ── Anexo: escolher / remover ──────────────────────────────────────────────
  // Define o anexo (do picker ou do paste): troca o preview e zera o erro.
  function setAttachment(f: File) {
    if (attachmentUrl) URL.revokeObjectURL(attachmentUrl);
    attachment = f;
    attachmentUrl = URL.createObjectURL(f);
    attachError = '';
  }

  function onPickFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0];
    if (f) setAttachment(f);
  }

  // Colar imagem na textarea (desktop garante; iOS Safari e instavel). Pega o 1o item de
  // imagem do clipboard e joga no mesmo fluxo do anexo.
  function onPaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const it of items) {
      if (it.kind === 'file' && it.type.startsWith('image/')) {
        const f = it.getAsFile();
        if (f) {
          e.preventDefault();
          setAttachment(f);
        }
        return;
      }
    }
  }

  function removeAttachment() {
    if (attachmentUrl) URL.revokeObjectURL(attachmentUrl);
    attachment = null;
    attachmentUrl = null;
    attachError = '';
    if (fileInput) fileInput.value = '';
  }

  async function submit() {
    if (!canSend) return;
    const caption = inputText.trim();
    if (attachment) {
      uploading = true;
      attachError = '';
      try {
        const { path } = await uploadImage(sessionName, attachment);
        // Uma linha so: o backend rejeita '\n' (control char) no send-keys -> evita o 500 que
        // comia a msg com legenda. O path nao tem espaco (nome gerado), entao da pra ler o trecho.
        const msg = (caption ? caption + ' — ' : '') + '📎 imagem: ' + path;
        inputText = '';
        if (textareaEl) textareaEl.style.height = 'auto';
        removeAttachment();
        onSend(msg);
      } catch (err) {
        attachError = err instanceof Error ? err.message : 'Falha no upload';
      } finally {
        uploading = false;
      }
      return;
    }
    const msg = caption;
    inputText = '';
    if (textareaEl) textareaEl.style.height = 'auto';
    onSend(msg);
  }

  // Auto-focus quando ocioso
  $effect(() => {
    if (sessionState === 'idle' && textareaEl) {
      setTimeout(() => textareaEl?.focus(), 100);
    }
  });
</script>

<footer class="composer">
  <input
    type="file"
    accept="image/*"
    bind:this={fileInput}
    onchange={onPickFile}
    class="file-input"
    aria-hidden="true"
    tabindex="-1"
  />
  <div class="composer-card">
    {#if typeof status?.costUsd === 'number'}
      <div class="composer-top">
        <button class="cost-chip" onclick={onExpandUsage} aria-label="Custo e uso">
          ${status.costUsd.toFixed(2)}
        </button>
      </div>
    {/if}

    {#if attachment}
      <div class="attach-chip">
        {#if attachmentUrl}<img class="attach-thumb" src={attachmentUrl} alt="anexo" />{/if}
        <span class="attach-name">{attachment.name}</span>
        {#if uploading}<span class="attach-status">enviando…</span>{/if}
        {#if attachError}<span class="attach-error">{attachError}</span>{/if}
        <button class="attach-remove" onclick={removeAttachment} aria-label="Remover anexo">×</button>
      </div>
    {/if}

    <SlashSuggest {commands} query={inputText} onPick={handleSuggestPick} />

    <textarea
      bind:this={textareaEl}
      bind:value={inputText}
      class="composer-textarea"
      placeholder="Mensagem para Claude…"
      rows={1}
      oninput={handleInput}
      onkeydown={handleKeydown}
      onpaste={onPaste}
      aria-label="Mensagem"
    ></textarea>

    <div class="control-row">
      <div class="control-left">
        <ContextRing pct={status?.ctxPct ?? null} />
        <button
          class="model-pill"
          onclick={() => (sheetOpen = true)}
          aria-label="Modelo e esforço de raciocínio"
        >
          {pillText}
        </button>
        <button class="attach-btn" onclick={() => fileInput?.click()} aria-label="Anexar imagem">
          <span class="attach-glyph" aria-hidden="true">📎</span>
        </button>
        <button
          class="slash-btn"
          onclick={() => (commandSheetOpen = true)}
          aria-label="Comandos"
        >
          <span class="slash-glyph" aria-hidden="true">/</span>
        </button>
      </div>

      <div class="control-right">
        {#if isWorking}
          <button class="stop-btn" onclick={() => (confirmStopOpen = true)} aria-label="Interromper Claude">
            <IconInterrupt size={16} />
          </button>
        {/if}
        <button
          class="send-btn"
          class:send-btn--disabled={!canSend}
          onclick={submit}
          disabled={!canSend}
          aria-label="Enviar mensagem"
        >
          <IconSend size={18} />
        </button>
      </div>
    </div>
  </div>

  <ModelEffortSheet
    open={sheetOpen}
    currentModel={pillModel}
    currentEffort={pillEffort}
    onApply={handleApply}
    onClose={() => (sheetOpen = false)}
  />

  <CommandSheet
    open={commandSheetOpen}
    {commands}
    onCommand={runCommand}
    onFill={fillCommand}
    onOpenModelEffort={() => (sheetOpen = true)}
    onClose={() => (commandSheetOpen = false)}
  />

  <ConfirmSheet
    open={confirmStopOpen}
    title="Interromper o Claude?"
    message="Isso envia ESC e para a resposta atual."
    confirmLabel="Interromper"
    danger={true}
    onConfirm={onInterrupt}
    onClose={() => (confirmStopOpen = false)}
  />
</footer>

<style>
  .composer {
    background: var(--bg-base);
    padding: var(--space-2) var(--space-3) var(--space-3);
  }

  /* Card unico que reune status, textarea e controles. */
  .composer-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 600px;
    margin: 0 auto;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    padding: var(--space-3);
  }

  /* ── Textarea (transparente dentro do card) ─────────────────────────────── */
  .composer-textarea {
    width: 100%;
    min-height: 24px;
    max-height: 120px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    line-height: 1.55;
    padding: var(--space-1) 0;
    resize: none;
    outline: none;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .composer-textarea::placeholder {
    color: var(--text-muted);
  }

  /* ── Control row ────────────────────────────────────────────────────────── */
  .control-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    min-height: 44px;
  }

  .control-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  /* Chip de modelo/esforco: botao que abre o ModelEffortSheet (mantem o visual do chip).
     min-height:0 sobrescreve o alvo global de 44px pra preservar o pill compacto de 28px
     (o tap fica confortavel dentro da control-row de 44px). :active scale vem do global. */
  .model-pill {
    display: inline-flex;
    align-items: center;
    height: 28px;
    min-height: 0;
    padding: 0 var(--space-3);
    background: var(--accent-dim);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 160px;
    font-variant-numeric: tabular-nums;
  }

  /* Botao [ / ]: abre o CommandSheet. Alvo de 44px, visual leve (so feedback no :active). */
  .slash-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-secondary);
  }

  .slash-btn:active {
    background: var(--bg-hover);
  }

  .slash-glyph {
    font-family: var(--font-mono);
    font-size: var(--text-lg);
    font-weight: 600;
    line-height: 1;
  }

  .control-right {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .stop-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: transparent;
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
    color: var(--error);
    transition: background 180ms var(--ease-out);
  }

  .stop-btn:active {
    background: rgba(255, 69, 58, 0.08);
  }

  /* Linha fina no topo do card: custo alinhado a direita, fora do control-row (libera o pill). */
  .composer-top {
    display: flex;
    justify-content: flex-end;
  }

  .cost-chip {
    display: inline-flex;
    align-items: center;
    height: 28px;
    min-height: 0;
    padding: 0 var(--space-2);
    background: var(--bg-hover);
    border-radius: var(--radius-md);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .send-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    transition: background 180ms var(--ease-out);
  }

  .send-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .send-btn--disabled {
    background: var(--bg-hover);
    color: var(--text-muted);
    cursor: default;
  }

  /* ── Anexo de imagem ────────────────────────────────────────────────────── */
  .file-input { display: none; }

  .attach-chip {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--bg-hover);
    border-radius: var(--radius-md);
  }
  .attach-thumb {
    width: 36px;
    height: 36px;
    border-radius: var(--radius-sm);
    object-fit: cover;
    flex-shrink: 0;
  }
  .attach-name {
    font-size: var(--text-xs);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
  }
  .attach-status { font-size: var(--text-xs); color: var(--text-muted); }
  .attach-error { font-size: var(--text-xs); color: var(--error); }
  .attach-remove {
    width: 28px; height: 28px; min-height: 0; flex-shrink: 0;
    color: var(--text-secondary); font-size: var(--text-lg); line-height: 1;
  }

  .attach-btn {
    width: 44px; height: 44px; flex-shrink: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-secondary);
  }
  .attach-btn:active { background: var(--bg-hover); }
  .attach-glyph { font-size: var(--text-lg); line-height: 1; }
</style>
