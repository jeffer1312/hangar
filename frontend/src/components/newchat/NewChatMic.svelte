<script lang="ts">
  import { onDestroy } from 'svelte';
  import { transcribeDictationForServer, type Server } from '@hangar/core';
  import { ditadoEstilo } from '../../lib/ditadoEstilo.svelte';
  import * as m from '../../paraglide/messages';

  // Microfone da conversa que ainda vai nascer: grava, o servidor só transcreve e limpa (rota sem
  // sessão) e o texto entra no campo. Mesma regra do `useHomeDictation` do app.
  interface Props { server: Server | null; disabled?: boolean; onText: (text: string) => void; onError: (msg: string) => void }
  let { server, disabled = false, onText, onError }: Props = $props();

  let recording = $state(false);
  let transcribing = $state(false);
  let recorder: MediaRecorder | null = null;
  let stream: MediaStream | null = null;
  let chunks: Blob[] = [];

  function release() {
    stream?.getTracks().forEach((t) => t.stop());
    stream = null;
    recorder = null;
  }
  onDestroy(release);

  async function transcribe(file: File) {
    if (!server) return;
    transcribing = true;
    try {
      const { text, aviso } = await transcribeDictationForServer(server, file, ditadoEstilo.valor);
      const trimmed = text.trim();
      if (!trimmed) throw new Error(m.composer_transcricao_vazia());
      onText(trimmed);
      // A limpeza pode desistir e devolver o cru: isso aparece, não passa por limpo.
      if (aviso) onError(aviso);
    } catch (e) {
      const detail = e instanceof Error ? e.message : m.composer_falha_transcricao();
      onError(/^(501|503):/.test(detail) ? `${m.composer_ditado_indisponivel()}: ${detail}` : detail);
    } finally {
      transcribing = false;
    }
  }

  async function toggle() {
    if (recording) { recorder?.stop(); return; }
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      onError(e instanceof Error && e.message ? e.message : m.composer_falha_gravacao());
      return;
    }
    chunks = [];
    const rec = new MediaRecorder(stream);
    recorder = rec;
    rec.ondataavailable = (e) => { if (e.data.size) chunks.push(e.data); };
    rec.onstop = () => {
      recording = false;
      // Chrome grava webm/opus; iOS Safari grava mp4/aac.
      const type = rec.mimeType || 'audio/webm';
      const ext = type.includes('mp4') ? 'm4a' : type.includes('ogg') ? 'ogg' : 'webm';
      const file = new File([new Blob(chunks, { type })], `gravacao-${Date.now()}.${ext}`, { type });
      release();
      if (chunks.length) void transcribe(file);
      else onError(m.composer_falha_gravacao());
    };
    rec.start();
    recording = true;
  }
</script>

<button type="button" class="mic" class:on={recording} aria-pressed={recording}
  aria-label={recording ? m.newchat_mic_stop() : m.newchat_mic()} disabled={disabled || transcribing || !server} onclick={toggle}>
  {#if transcribing}
    <span class="spin" aria-hidden="true"></span>
  {:else}
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="9" y="2" width="6" height="12" rx="3" /><path d="M5 10v1a7 7 0 0 0 14 0v-1" /><line x1="12" y1="18" x2="12" y2="22" />
    </svg>
  {/if}
</button>

<style>
  .mic {
    width: 44px; height: 44px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center;
    border: 0; border-radius: var(--radius-full); background: transparent; color: var(--text-secondary); cursor: pointer;
  }
  .mic.on { background: var(--error); color: #fff; }
  .mic:disabled { opacity: 0.5; cursor: default; }
  .spin {
    width: 16px; height: 16px; border-radius: 50%; border: 2px solid currentColor; border-top-color: transparent;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
