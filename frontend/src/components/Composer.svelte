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
    onSend: (text: string) => Promise<void> | void;
    onCommand: (cmd: string) => void;
    onInterrupt: () => void;
    onExpandUsage: () => void;
    scrolling?: boolean;
  }
  let { sessionName, sessionState, status, onSend, onCommand, onInterrupt, onExpandUsage, scrolling }: Props = $props();

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

  // ── Anexos de imagem: lista de arquivos + preview local + estado de upload ──
  let attachments = $state<{ file: File; url: string }[]>([]);
  let fileInput: HTMLInputElement | undefined = $state();
  let uploading = $state(false);
  let attachError = $state('');
  let sending = $state(false);
  let sendError = $state('');

  // hasInput: tem texto OU anexo. Usado tb pro botao stop/send (ver control-right).
  const hasInput = $derived(inputText.trim().length > 0 || attachments.length > 0);
  const canSend = $derived(hasInput && !uploading && !sending);
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
    sendError = '';
    autoGrow();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  // ── Anexos: escolher / remover (multiplas imagens) ─────────────────────────
  // Adiciona arquivos de imagem a lista (do picker ou do paste), cada um com preview local.
  function addFiles(files: Iterable<File>) {
    for (const f of files) {
      if (!f.type.startsWith('image/')) continue;
      attachments = [...attachments, { file: f, url: URL.createObjectURL(f) }];
    }
    attachError = '';
  }

  function onPickFile(e: Event) {
    const files = (e.target as HTMLInputElement).files;
    if (files && files.length) addFiles(files);
  }

  // Colar imagem(ns) (desktop garante; iOS Safari e instavel). Pega todos os itens de imagem
  // do clipboard e joga no mesmo fluxo do anexo.
  function onPaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imgs: File[] = [];
    for (const it of items) {
      if (it.kind === 'file' && it.type.startsWith('image/')) {
        const f = it.getAsFile();
        if (f) imgs.push(f);
      }
    }
    if (imgs.length) {
      e.preventDefault();
      addFiles(imgs);
    }
  }

  // Remove um anexo pelo indice (libera o objectURL).
  function removeAttachment(idx: number) {
    const a = attachments[idx];
    if (a) URL.revokeObjectURL(a.url);
    attachments = attachments.filter((_, i) => i !== idx);
    attachError = '';
    if (fileInput) fileInput.value = '';
  }

  // Limpa todos os anexos (apos envio com sucesso).
  function clearAttachments() {
    for (const a of attachments) URL.revokeObjectURL(a.url);
    attachments = [];
    if (fileInput) fileInput.value = '';
  }

  async function submit() {
    if (!canSend) return;
    const caption = inputText.trim();
    sendError = '';
    if (attachments.length) {
      uploading = true;
      attachError = '';
      try {
        // Sobe todas as imagens e junta os paths numa UNICA linha (o backend rejeita '\n' no
        // send-keys). Cada path nao tem espaco (nome gerado) -> separa por " 📎 imagem: ".
        const paths: string[] = [];
        for (const a of attachments) {
          const { path } = await uploadImage(sessionName, a.file);
          paths.push(path);
        }
        const imgPart = paths.map((p) => '📎 imagem: ' + p).join(' ');
        const msg = (caption ? caption + ' — ' : '') + imgPart;
        await onSend(msg);                 // espera o /input; so limpa se foi
        inputText = '';
        if (textareaEl) textareaEl.style.height = 'auto';
        clearAttachments();
      } catch (err) {
        // upload OU envio falhou -> mantem as fotos e o texto, mostra o erro.
        attachError = err instanceof Error ? err.message : 'Falha no envio';
      } finally {
        uploading = false;
      }
      return;
    }
    // texto puro: espera o envio; so limpa no sucesso, mostra erro na falha.
    sending = true;
    try {
      await onSend(caption);
      inputText = '';
      if (textareaEl) textareaEl.style.height = 'auto';
    } catch (err) {
      sendError = err instanceof Error ? err.message : 'Falha no envio';
    } finally {
      sending = false;
    }
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
    multiple
    bind:this={fileInput}
    onchange={onPickFile}
    class="file-input"
    aria-hidden="true"
    tabindex="-1"
  />
  <div class="composer-card" class:scrolling={scrolling}>
    <div class="composer-top">
      <div class="top-left">
        <button class="slash-btn" onclick={() => (commandSheetOpen = true)} aria-label="Comandos">
          <span class="slash-glyph" aria-hidden="true">/</span>
        </button>
        {#if status?.repo}
          <span class="repo-chip" title="Pasta e branch">
            <span class="repo-glyph" aria-hidden="true">📁</span>
            <span class="repo-name">{status.repo}</span>
            {#if status.branch}
              <span class="repo-sep" aria-hidden="true">·</span>
              <span class="repo-branch">{status.branch}{#if status.dirty}<span class="repo-dirty" aria-label="alterações não commitadas">*</span>{/if}</span>
            {/if}
          </span>
        {/if}
      </div>
      {#if typeof status?.costUsd === 'number'}
        <button class="cost-chip" onclick={onExpandUsage} aria-label="Custo e uso">
          ${status.costUsd.toFixed(2)}
        </button>
      {/if}
    </div>

    {#if attachments.length}
      <div class="attach-row">
        {#each attachments as a, idx (a.url)}
          <div class="attach-chip">
            <img class="attach-thumb" src={a.url} alt="anexo" />
            <button class="attach-remove" onclick={() => removeAttachment(idx)} aria-label="Remover anexo">×</button>
          </div>
        {/each}
        {#if uploading}<span class="attach-status">enviando…</span>{/if}
        {#if attachError}<span class="attach-error">{attachError}</span>{/if}
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

    {#if sendError}
      <div class="send-error" role="alert">{sendError}</div>
    {/if}

    <div class="control-row">
      <div class="control-left">
        <button
          class="model-pill"
          onclick={() => (sheetOpen = true)}
          aria-label="Modelo, esforço e contexto"
        >
          <span class="pill-text">{pillText}</span>
          <ContextRing pct={status?.ctxPct ?? null} />
        </button>
        <button class="attach-btn" onclick={() => fileInput?.click()} aria-label="Anexar imagem">
          <span class="attach-glyph" aria-hidden="true">📎</span>
        </button>
      </div>

      <div class="control-right">
        {#if isWorking && !hasInput}
          <!-- Pensando + input vazio -> o slot vira STOP. Ao digitar/colar algo, volta a ser SEND
               (enfileira a msg). Um slot so -> ganha espaco. -->
          <button class="stop-btn" onclick={() => (confirmStopOpen = true)} aria-label="Interromper Claude">
            <IconInterrupt size={16} />
          </button>
        {:else}
          <button
            class="send-btn"
            class:send-btn--disabled={!canSend}
            onclick={submit}
            disabled={!canSend}
            aria-label="Enviar mensagem"
          >
            <IconSend size={18} />
          </button>
        {/if}
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
    background: transparent;
    padding: var(--space-2) var(--space-3) 0;
  }

  /* Card unico que reune status, textarea e controles. */
  .composer-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 600px;
    margin: 0 auto;
    /* Card de vidro fosco. O blur NÃO fica mais aqui: ficava no MESMO elemento que tem conteúdo +
       borda + filhos, forçando o WebKit a promover a subárvore e quebrar o fast-path do scroll ->
       bloco PRETO no topo durante o streaming (WebKit #89475). Agora o filtro vive só no ::before
       (leaf isolado). Host = stacking context próprio (position+isolation), transparente, só
       borda/sombra. Padrão de produção (Ionic .footer-background). Os sheets do Composer são IRMÃOS
       do card (fora dele) -> isolation/position aqui NÃO os clipa. */
    position: relative;
    isolation: isolate;
    border: 1px solid rgba(255, 255, 255, 0.10);
    box-shadow:               /* glow 0 12px 40px FICA no host (sem overflow:hidden) -> halo preservado */
      inset 0 1px 0 rgba(255, 255, 255, 0.14),
      inset 0 -1px 0 rgba(0, 0, 0, 0.22),
      0 1px 2px rgba(0, 0, 0, 0.18),
      0 12px 40px rgba(0, 0, 0, 0.42);
    border-radius: var(--radius-lg);
    padding: var(--space-3) var(--space-3) calc(var(--space-3) + env(safe-area-inset-bottom));
  }

  /* ÚNICA camada com backdrop-filter: leaf bare (sem conteúdo, sem descendente posicionado), bounded
     à caixa do dock -> não promove/corrompe a camada da tela. Mesmo gradiente+blur de antes. */
  .composer-card::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;                /* atrás do conteúdo, dentro do stacking context do card */
    border-radius: inherit;
    pointer-events: none;
    background:
      linear-gradient(180deg, rgba(255, 255, 255, 0.06), rgba(255, 255, 255, 0) 38%),
      rgba(28, 28, 32, 0.55);
    backdrop-filter: blur(30px) saturate(180%) brightness(1.05);
    -webkit-backdrop-filter: blur(30px) saturate(180%) brightness(1.05);
  }

  /* Rede de segurança durante o scroll: desliga o backdrop-filter e usa fundo quase opaco. MIGRADA
     pro ::before (senão sumia silenciosamente com o filtro tendo mudado de elemento). */
  .composer-card.scrolling::before {
    backdrop-filter: none;
    -webkit-backdrop-filter: none;
    background: rgba(28, 28, 32, 0.94);
  }

  /* Desktop: composer mais largo (aditivo; mobile fica nos 600px). */
  @media (min-width: 820px) {
    .composer-card { max-width: 920px; }
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
    gap: var(--space-2);
    height: 30px;
    min-height: 0;
    padding: 0 var(--space-2) 0 var(--space-3);
    background: var(--accent-dim);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .pill-text {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 130px;
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

  /* Linha fina no topo do card: slash a esquerda, custo a direita (fora do control-row). */
  .composer-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .top-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .repo-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .repo-glyph { font-size: 11px; flex-shrink: 0; }
  .repo-name {
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 120px;
  }
  .repo-sep { color: var(--text-muted); }
  .repo-branch {
    font-family: var(--font-mono);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 110px;
  }
  .repo-dirty { color: var(--warning); margin-left: 1px; }

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

  .attach-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .attach-chip {
    position: relative;
    flex-shrink: 0;
  }
  .attach-thumb {
    width: 48px;
    height: 48px;
    border-radius: var(--radius-sm);
    object-fit: cover;
    display: block;
  }
  .attach-status { font-size: var(--text-xs); color: var(--text-muted); }
  .attach-error { font-size: var(--text-xs); color: var(--error); }
  .attach-remove {
    position: absolute;
    top: -6px;
    right: -6px;
    width: 20px;
    height: 20px;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-full);
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    color: var(--text-secondary);
    font-size: 14px;
    line-height: 1;
  }

  .attach-btn {
    width: 44px; height: 44px; flex-shrink: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-secondary);
  }
  .attach-btn:active { background: var(--bg-hover); }
  .attach-glyph { font-size: var(--text-lg); line-height: 1; }

  .send-error {
    font-size: var(--text-xs);
    color: var(--error);
    padding: 0 var(--space-1);
  }
</style>
