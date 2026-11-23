import { useEffect, useState } from 'react';
import { Platform } from 'react-native';
import { createVideoPlayer, type VideoPlayer, type VideoThumbnail } from 'expo-video';
import { registrarDiag } from '@hangar/core';

// Quadros guardados enquanto o app vive; cada um é um bitmap nativo, por isso o teto.
const MAX_THUMBS = 40;
const MAX_WIDTH = 480;
// Vídeo que não responde não segura a fila das outras miniaturas.
const TIMEOUT_MS = 15000;

type Slot = { value: VideoThumbnail | null; done: boolean; wanted: number; waiters: ((v: VideoThumbnail | null) => void)[] };

const slots = new Map<string, Slot>();
const queue: { uri: string; headers?: Record<string, string> }[] = [];
let running = false;

// Os usados há mais tempo saem primeiro; o que ainda espera a vez fica.
function prune() {
  for (const [k, s] of slots) {
    if (slots.size <= MAX_THUMBS) break;
    if (s.done) slots.delete(k);
  }
}

function remember(uri: string, slot: Slot) {
  slots.delete(uri);
  slots.set(uri, slot);
  prune();
}

// iOS: o item do player só existe depois de a fonte carregar, e antes disso o pedido de quadro volta
// vazio na hora. O Android lê o quadro direto da fonte e não precisa esperar.
function sourceReady(player: VideoPlayer, subs: { remove(): void }[]): Promise<void> {
  if (Platform.OS !== 'ios' || player.status === 'readyToPlay') return Promise.resolve();
  if (player.status === 'error') return Promise.reject(new Error('video source failed'));
  return new Promise((resolve, reject) => {
    subs.push(player.addListener('statusChange', ({ status, error }) => {
      if (status === 'readyToPlay') resolve();
      else if (status === 'error') reject(new Error(error?.message ?? 'video source failed'));
    }));
  });
}

async function generate(uri: string, headers?: Record<string, string>): Promise<VideoThumbnail | null> {
  const player = createVideoPlayer({ uri, headers });
  const subs: { remove(): void }[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    // Um prazo só para carregar a fonte e tirar o quadro.
    const timeout = new Promise<null>((resolve) => { timer = setTimeout(() => resolve(null), TIMEOUT_MS); });
    const work = sourceReady(player, subs).then(() => player.generateThumbnailsAsync(0, { maxWidth: MAX_WIDTH }));
    const frames = await Promise.race([work, timeout]);
    return frames?.[0] ?? null;
  } catch (e) {
    // Sem quadro fica o ▶ com o nome; o visor ainda tenta tocar e mostra o erro de verdade.
    const detalhe = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
    registrarDiag({ evento: 'anexos.miniatura_falhou', nivel: 'aviso', tela: 'anexos', codigo: 'video_thumb_failed', detalhe: detalhe.slice(0, 300) });
    return null;
  } finally {
    clearTimeout(timer);
    for (const s of subs) s.remove();
    player.release();
  }
}

// Um vídeo por vez: cada miniatura abre um player e lê o começo do arquivo pela rede.
async function pump() {
  if (running) return;
  running = true;
  try {
    for (let job = queue.shift(); job; job = queue.shift()) {
      const slot = slots.get(job.uri);
      if (!slot || slot.done) continue;
      // Quem pediu saiu da tela antes da vez dele: não gasta rede, e a próxima montagem pede de novo.
      if (slot.wanted <= 0) { slots.delete(job.uri); continue; }
      const value = await generate(job.uri, job.headers);
      slot.value = value;
      slot.done = true;
      // Sem quadro (prazo, rede) não fica guardado: a próxima montagem tenta de novo.
      if (!value && slots.get(job.uri) === slot) slots.delete(job.uri);
      // Uma rajada de pedidos passa do teto enquanto todos esperam; ao concluir, o excesso sai.
      prune();
      for (const w of slot.waiters.splice(0)) w(value);
    }
  } finally {
    running = false;
  }
}

/** Primeiro quadro do vídeo para a miniatura da conversa; null enquanto não há (ou se não deu). */
export function useVideoThumb(uri: string, headers?: Record<string, string>): VideoThumbnail | null {
  // O quadro guarda o vídeo de onde veio: trocar de vídeo não mostra o quadro do anterior.
  const [thumb, setThumb] = useState<{ uri: string; value: VideoThumbnail | null }>(() => ({ uri, value: slots.get(uri)?.value ?? null }));
  useEffect(() => {
    let slot = slots.get(uri);
    if (slot?.done) {
      remember(uri, slot);
      setThumb({ uri, value: slot.value });
      return;
    }
    if (!slot) {
      slot = { value: null, done: false, wanted: 0, waiters: [] };
      remember(uri, slot);
      queue.push({ uri, headers });
    }
    const s = slot;
    let alive = true;
    s.wanted++;
    s.waiters.push((v) => { if (alive) setThumb({ uri, value: v }); });
    void pump();
    return () => { alive = false; s.wanted--; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [uri]);
  return thumb.uri === uri ? thumb.value : null;
}
