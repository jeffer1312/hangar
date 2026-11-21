import { useEffect, useState } from 'react';
import { createVideoPlayer, type VideoThumbnail } from 'expo-video';

// Quadros guardados enquanto o app vive; cada um é um bitmap nativo, por isso o teto.
const MAX_THUMBS = 40;
const MAX_WIDTH = 480;
// Vídeo que não responde não segura a fila das outras miniaturas.
const TIMEOUT_MS = 15000;

type Slot = { value: VideoThumbnail | null; done: boolean; wanted: number; waiters: ((v: VideoThumbnail | null) => void)[] };

const slots = new Map<string, Slot>();
const queue: { uri: string; headers?: Record<string, string> }[] = [];
let running = false;

function remember(uri: string, slot: Slot) {
  slots.delete(uri);
  slots.set(uri, slot);
  for (const [k, s] of slots) {
    if (slots.size <= MAX_THUMBS) break;
    if (s.done) slots.delete(k);
  }
}

async function generate(uri: string, headers?: Record<string, string>): Promise<VideoThumbnail | null> {
  const player = createVideoPlayer({ uri, headers });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const timeout = new Promise<null>((resolve) => { timer = setTimeout(() => resolve(null), TIMEOUT_MS); });
    const frames = await Promise.race([player.generateThumbnailsAsync(0, { maxWidth: MAX_WIDTH }), timeout]);
    return frames?.[0] ?? null;
  } catch {
    // Sem quadro fica o ▶ com o nome; o visor ainda tenta tocar e mostra o erro de verdade.
    return null;
  } finally {
    clearTimeout(timer);
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
      for (const w of slot.waiters.splice(0)) w(value);
    }
  } finally {
    running = false;
  }
}

/** Primeiro quadro do vídeo para a miniatura da conversa; null enquanto não há (ou se não deu). */
export function useVideoThumb(uri: string, headers?: Record<string, string>): VideoThumbnail | null {
  const [thumb, setThumb] = useState<VideoThumbnail | null>(() => slots.get(uri)?.value ?? null);
  useEffect(() => {
    let slot = slots.get(uri);
    if (slot?.done) {
      remember(uri, slot);
      setThumb(slot.value);
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
    s.waiters.push((v) => { if (alive) setThumb(v); });
    void pump();
    return () => { alive = false; s.wanted--; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [uri]);
  return thumb;
}
