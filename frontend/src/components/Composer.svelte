<script module lang="ts">
  import type { CommandInfo } from '../lib/types';
  // Cache de comandos por sessao: sobrevive a remontagens do Composer (ex: voltar de
  // awaiting_input) pra buscar a lista so uma vez por sessao.
  const commandCache = new Map<string, CommandInfo[]>();
</script>

<script lang="ts">
  import { tick, onDestroy } from 'svelte';
  import IconSend from './icons/IconSend.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';
  import IconAttach from './icons/IconAttach.svelte';
  import IconMic from './icons/IconMic.svelte';
  import ContextRing from './ContextRing.svelte';
  import ModelEffortSheet from './ModelEffortSheet.svelte';
  import SlashSuggest from './SlashSuggest.svelte';
  import CommandSheet from './CommandSheet.svelte';
  import ConfirmSheet from './ConfirmSheet.svelte';
  import { getCommands, setModelEffort, uploadFile, transcribeFile, type ModelEffortBody } from '../lib/api';
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
    onOpenGit: () => void;
    onOpenPreview: () => void;
    inputText?: string;  // bindable: o pai injeta um draft (ex: interrupt devolve a msg pendente)
  }
  let {
    sessionName, sessionState, status, onSend, onCommand, onInterrupt, onExpandUsage, onOpenGit,
    onOpenPreview,
    inputText = $bindable(''),
  }: Props = $props();

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

  let textareaEl: HTMLTextAreaElement | undefined = $state();

  // Exposto pro pai (atalho de teclado desktop "/" foca o campo).
  export function focus() { textareaEl?.focus(); }

  // ── Anexos: lista de arquivos + preview local + estado de upload ────────────
  // isAudio -> transcrito via Groq no envio (ver submit); isImage -> preview; resto -> chip de arquivo.
  let attachments = $state<{ file: File; url: string; isImage: boolean; isAudio: boolean }[]>([]);
  let fileInput: HTMLInputElement | undefined = $state();
  let uploading = $state(false);
  let attachError = $state('');
  let sending = $state(false);
  let sendError = $state('');

  // ── Gravacao de audio pelo microfone (MediaRecorder) ────────────────────────
  let recording = $state(false);
  let recError = $state('');
  let mediaRecorder: MediaRecorder | undefined;
  let recChunks: Blob[] = [];
  let recStream: MediaStream | undefined;
  let recFailed = false;   // marcado no onerror -> onstop nao anexa audio truncado
  let starting = false;    // guarda reentrancia entre o tap e o await getUserMedia resolver

  // hasInput: tem texto OU anexo. Usado tb pro botao stop/send (ver control-right).
  const hasInput = $derived(inputText.trim().length > 0 || attachments.length > 0);
  const canSend = $derived(hasInput && !uploading && !sending && !recording);
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

  // Tap em qualquer area do composer que nao seja um controle -> foca o input.
  // .focus() dentro do gesto de clique levanta o teclado no mobile.
  function focusInput(e: MouseEvent) {
    const t = e.target as HTMLElement;
    if (t.closest('button, a, input, textarea, [role="option"], [role="listbox"]')) return;
    textareaEl?.focus();
  }

  function handleKeydown(e: KeyboardEvent) {
    // Enter-envia SO no desktop (hover + pointer fine). No teclado do celular, Enter QUEBRA LINHA
    // (comportamento nativo do textarea): enviar era facil demais de disparar sem querer — no
    // mobile o envio e pelo botao. Shift+Enter segue quebrando linha no desktop. Checado na hora
    // (nao cacheado): tablet que pluga/despluga teclado troca de modo sem reload.
    if (e.key === 'Enter' && !e.shiftKey
        && window.matchMedia('(hover: hover) and (pointer: fine)').matches) {
      e.preventDefault();
      submit();
    }
  }

  // ── Anexos: escolher / remover (multiplas imagens) ─────────────────────────
  // Adiciona arquivos de imagem a lista (do picker ou do paste), cada um com preview local.
  function addFiles(files: Iterable<File>) {
    for (const f of files) {
      const isImage = f.type.startsWith('image/');
      // audio (arquivo audio/* ou gravacao do mic) -> transcrito no envio. url so pra preview de
      // imagem; outros tipos viram chip com o nome (sem objectURL pra revogar).
      const isAudio = f.type.startsWith('audio/');
      attachments = [...attachments, { file: f, url: isImage ? URL.createObjectURL(f) : '', isImage, isAudio }];
    }
    attachError = '';
  }

  // ── Gravar audio: toggle (tap grava, tap para) -> vira um anexo de audio ─────
  // Para o stream do mic e zera o estado. Chamado no onstop, no onerror, em falha e no onDestroy
  // (trocar de sessao com gravacao ativa desmonta o Composer -> sem isto o mic ficaria ligado).
  function teardownRecording() {
    recStream?.getTracks().forEach((t) => t.stop());
    recStream = undefined;
    mediaRecorder = undefined;
    recording = false;
  }

  async function toggleRecord() {
    if (recording) {
      // Guard: so para se ainda esta gravando. Duplo-toque no stop chamaria .stop() num recorder ja
      // 'inactive' -> InvalidStateError nao tratado.
      if (mediaRecorder?.state === 'recording') mediaRecorder.stop();
      return;
    }
    if (starting) return;   // 2o tap na janela do await getUserMedia abriria um 2o stream (vaza o mic)
    starting = true;
    recError = '';
    recFailed = false;
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (err) {
      console.error('getUserMedia falhou', err);
      const name = err instanceof DOMException ? err.name : '';
      recError = name === 'NotFoundError' ? 'Nenhum microfone encontrado'
        : name === 'NotReadableError' ? 'Microfone em uso por outro app'
        : 'Sem acesso ao microfone';
      starting = false;
      return;
    }
    recStream = stream;
    recChunks = [];
    // Construcao/wiring/start dentro do try: mimeType nao suportado ou API ausente lancam aqui —
    // sem isto, a falha some e o mic fica ligado (stream nunca parado).
    try {
      mediaRecorder = new MediaRecorder(stream);
      mediaRecorder.ondataavailable = (e) => { if (e.data.size) recChunks.push(e.data); };
      mediaRecorder.onstop = () => {
        const type = mediaRecorder?.mimeType || 'audio/webm';
        // Chrome grava webm/opus; iOS Safari grava mp4/aac. A Groq aceita os dois direto.
        const ext = type.includes('mp4') ? 'm4a' : type.includes('ogg') ? 'ogg' : 'webm';
        // onerror dispara stop logo depois -> se ja falhou, nao anexa o audio (truncado). Sem chunk
        // nenhum (gravacao rapida demais / driver sem dado) -> avisa, nao some calado.
        if (recFailed) {
          teardownRecording();
        } else if (recChunks.length) {
          const blob = new Blob(recChunks, { type });
          addFiles([new File([blob], `gravacao-${Date.now()}.${ext}`, { type })]);
          teardownRecording();
        } else {
          recError = 'Gravação vazia, tente de novo';
          teardownRecording();
        }
      };
      mediaRecorder.onerror = (e) => {
        console.error('MediaRecorder erro', (e as { error?: unknown }).error ?? e);
        recFailed = true;
        recError = 'Falha na gravação';
        teardownRecording();
      };
      mediaRecorder.start();
    } catch (err) {
      console.error('MediaRecorder falhou', err);
      recError = 'Gravação de áudio não suportada neste navegador';
      teardownRecording();
      starting = false;
      return;
    }
    recording = true;
    starting = false;
  }

  onDestroy(teardownRecording);

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
    if (a?.url) URL.revokeObjectURL(a.url);
    attachments = attachments.filter((_, i) => i !== idx);
    attachError = '';
    if (fileInput) fileInput.value = '';
  }

  // Limpa todos os anexos (apos envio com sucesso).
  function clearAttachments() {
    for (const a of attachments) if (a.url) URL.revokeObjectURL(a.url);
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
        // Sobe todos os anexos e junta os paths numa UNICA linha (o backend rejeita '\n' no
        // send-keys). Cada path nao tem espaco (nome gerado). Marca imagem x arquivo pelo tipo.
        const parts: string[] = [];
        for (const a of attachments) {
          if (a.isAudio) {
            // audio: backend salva + transcreve (Groq) -> texto em UMA linha + path do audio anexado.
            const { path, text } = await transcribeFile(sessionName, a.file);
            parts.push((text ? text + ' — ' : '') + '📎 áudio: ' + path);
            continue;
          }
          const { path } = await uploadFile(sessionName, a.file);
          parts.push((a.isImage ? '📎 imagem: ' : '📎 arquivo: ') + path);
        }
        const attachPart = parts.join(' ');
        const msg = (caption ? caption + ' — ' : '') + attachPart;
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
    // texto puro: limpa a caixa OTIMISTA (no toque) pra tirar a sensacao de lag do round-trip; restaura
    // o texto se o /input falhar (nao perde nada). O settle ~200ms do backend (anti-"Enter engolido")
    // continua valendo no servidor, mas a UI nao bloqueia mais a limpeza nele.
    sending = true;
    inputText = '';
    if (textareaEl) textareaEl.style.height = 'auto';
    try {
      await onSend(caption);
    } catch (err) {
      // falhou -> devolve o texto, MAS so se a caixa segue vazia (nao pisa no que o usuario digitou
      // na janela do envio em voo).
      if (!inputText.trim()) inputText = caption;
      sendError = err instanceof Error ? err.message : 'Falha no envio';
    } finally {
      sending = false;
    }
  }

  // Auto-focus quando ocioso — SO em desktop (ponteiro fino). No iOS, focus() programatico nao abre
  // o teclado (so abre com gesto real) mas mexe no estado de foco do campo -> briga com o tap do
  // usuario e o teclado so abre depois de varios toques. matchMedia('(pointer: fine)') = mouse/trackpad
  // (desktop), exclui touch.
  $effect(() => {
    if (sessionState === 'idle' && textareaEl && window.matchMedia('(pointer: fine)').matches) {
      setTimeout(() => textareaEl?.focus(), 100);
    }
  });
</script>

<footer class="composer">
  <input
    type="file"
    accept="*/*"
    multiple
    bind:this={fileInput}
    onchange={onPickFile}
    class="file-input"
    aria-hidden="true"
    tabindex="-1"
  />
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="composer-card" onclick={focusInput}>
    <div class="composer-top">
      <div class="top-left">
        <button class="slash-btn" onclick={() => (commandSheetOpen = true)} aria-label="Comandos">
          <span class="slash-glyph" aria-hidden="true">/</span>
        </button>
        <button class="slash-btn" onclick={onOpenPreview} aria-label="Preview de projeto rodando">
          <span class="slash-glyph" aria-hidden="true">🖥</span>
        </button>
        {#if status?.repo}
          <button class="repo-chip" title="Git: trocar branch / status / pull" onclick={onOpenGit}>
            <span class="repo-glyph" aria-hidden="true">📁</span>
            <span class="repo-name">{status.repo}</span>
            {#if status.branch}
              <span class="repo-sep" aria-hidden="true">·</span>
              <span class="repo-branch">{status.branch}{#if status.dirty}<span class="repo-dirty" aria-label="alterações não commitadas">*</span>{/if}</span>
            {/if}
          </button>
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
        {#each attachments as a, idx (a.file)}
          <div class="attach-chip">
            {#if a.isImage}
              <img class="attach-thumb" src={a.url} alt="anexo" />
            {:else}
              <span class="attach-file" title={a.file.name}>
                <span class="attach-file-glyph" aria-hidden="true">{a.isAudio ? '🎤' : '📎'}</span>
                <span class="attach-file-name">{a.isAudio ? 'áudio' : a.file.name}</span>
              </span>
            {/if}
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

    {#if recording}
      <div class="rec-hint" role="status"><span class="rec-dot" aria-hidden="true"></span> gravando… toque ⏹ para parar</div>
    {/if}
    {#if recError}
      <div class="send-error" role="alert">{recError}</div>
    {/if}
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
          <span class="pill-label">
            <span class="pill-model">{pillModel ?? 'Modelo'}</span>
            {#if pillEffort}<span class="pill-effort">· {pillEffort}</span>{/if}
          </span>
          <ContextRing pct={status?.ctxPct ?? null} />
        </button>
        <button class="attach-btn" onclick={() => fileInput?.click()} aria-label="Anexar arquivo">
          <IconAttach size={20} />
        </button>
        <button
          class="attach-btn mic-btn"
          class:mic-btn--recording={recording}
          onclick={toggleRecord}
          aria-label={recording ? 'Parar gravação' : 'Gravar áudio'}
        >
          {#if recording}<IconInterrupt size={18} />{:else}<IconMic size={20} />{/if}
        </button>
      </div>

      <div class="control-right">
        {#if isWorking && !hasInput}
          <!-- Pensando + input vazio -> o slot vira STOP. Ao digitar/colar algo, volta a ser SEND
               (enfileira a msg). Um slot so -> ganha espaco. -->
          <button class="stop-btn" onclick={() => (confirmStopOpen = true)} aria-label="Interromper Claude">
            <IconInterrupt size={18} />
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
    /* Folga do home indicator vive AQUI (externa) -> o card flutua com respiro do fundo, sem
       colar na borda da tela. max() = só a folga do indicator quando há, senão o space-2 mínimo. */
    padding: var(--space-2) var(--space-3) var(--composer-pb, max(var(--space-2), env(safe-area-inset-bottom)));
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
    border: 1px solid var(--glass-border);
    box-shadow:               /* specular rim (brilho de borda) = cara de glass iOS; glow no host */
      inset 0 1px 1px var(--glass-specular),
      inset 0 -1px 1px rgba(255, 255, 255, 0.05),
      0 1px 2px rgba(0, 0, 0, 0.18),
      0 12px 40px var(--glass-shadow);
    border-radius: var(--radius-lg);
    /* Padding interno uniforme. A folga do home indicator saiu daqui pro .composer (margem externa)
       -> o card nao cola mais na borda; flutua com respiro do fundo. */
    padding: var(--space-3);
  }

  /* Camada de vidro: leaf bare (sem conteúdo, sem descendente posicionado), bounded à caixa do dock. */
  .composer-card::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;                /* atrás do conteúdo, dentro do stacking context do card */
    border-radius: inherit;
    pointer-events: none;
    /* WebKit/iOS: SEM backdrop-filter. Tira o blur(40px) e com ele o bug #89475 (bloco preto no
       streaming/momentum) de vez — sem filtro, nada pra promover/corromper. Fundo quase opaco = o
       look de "vidro" que antes só aparecia no scroll, agora permanente. */
    background: var(--glass-bg-solid);
  }
  /* Chromium (data-liquid): refracao SVG real (liquid glass). O blur fica aqui — Chromium não tem o
     bug do WebKit. Fundo transparente pra refração aparecer. */
  :global(html[data-liquid]) .composer-card::before {
    background: var(--glass-bg);
    backdrop-filter: url(#liquid-glass) blur(16px) saturate(180%);
  }

  /* Desktop: composer mais largo (aditivo; mobile fica nos 600px). min() acompanha a lista
     (messages-inner): sem degrau 600->1400 em tablet. */
  @media (min-width: 820px) {
    .composer-card { max-width: min(1400px, 94vw); }
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

  .pill-label {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    min-width: 0;
  }

  .pill-model {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 130px;
    color: var(--text-primary);
    font-weight: 600;
  }

  .pill-effort {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  /* Botao [ / ]: abre o CommandSheet. Chip compacto na faixa do topo, igual ao cost-chip.
     min-height/min-width:0 sobrescrevem o alvo global de 44px pra manter o chip enxuto
     (tap confortavel dentro da faixa). */
  .slash-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 28px;
    min-height: 0;
    min-width: 0;
    padding: 0 var(--space-2);
    flex-shrink: 0;
    border-radius: var(--radius-md);
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  .slash-btn:active {
    background: var(--bg-elevated);
  }

  .slash-glyph {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
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
    background: var(--bg-elevated);
    border-radius: var(--radius-md);
    color: var(--error);
    transition: background 180ms var(--ease-out);
  }

  .stop-btn:active {
    background: var(--bg-hover);
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
    appearance: none;
    border: 0;
    background: transparent;
    padding: 0;
    cursor: pointer;
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
  .attach-file {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 140px;
    height: 48px;
    padding: 0 var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
  }
  .attach-file-glyph { flex-shrink: 0; font-size: 13px; }
  .attach-file-name {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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
  .attach-btn :global(svg) { display: block; }

  /* Botao de gravar audio: ícone de mic (IconMic) / quadrado stop (IconInterrupt) enquanto grava.
     Gravando -> vermelho e pulsa. */
  .mic-btn--recording { color: var(--error); animation: mic-pulse 1.2s var(--ease-out) infinite; }
  @keyframes mic-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.45; }
  }

  /* Aviso "gravando…" com bolinha vermelha piscando. */
  .rec-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--text-xs);
    color: var(--text-secondary);
    padding: 0 var(--space-1);
  }
  .rec-dot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-full);
    background: var(--error);
    animation: mic-pulse 1.2s var(--ease-out) infinite;
  }

  .send-error {
    font-size: var(--text-xs);
    color: var(--error);
    padding: 0 var(--space-1);
  }
</style>
