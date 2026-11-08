// Ditado em pedaços durante a fala (iPhone): a regra de corte do core decide onde cortar pelos níveis do
// medidor; cada pedaço sai do WAV que o gravador ainda está escrevendo e vai só para a transcrição.
// A organização acontece uma vez, no fim, em `dictationRun`. Qualquer problema desiste dos pedaços e o
// ditado segue o caminho de hoje, com o arquivo inteiro.
import {
  feedDictationFrame, joinDictationPieces, newDictationCutter, stopDictationCutter, transcribePiece,
  type DictationPiece, type Server,
} from '@hangar/core';

/** 16 kHz mono PCM16. */
export const PCM_BYTES_PER_MS = 32;
/** Pedaço cortado cujos dados não chegam ao arquivo por este tempo: o gravador parou de escrever. */
export const STALL_MS = 3000;
/** O iPhone põe os dados no byte 4096; lê-se um pouco além para achar o `data`. */
const HEADER_READ = 8192;

/** O WAV que o gravador está escrevendo. */
export interface GrowingFile {
  size(): number;
  read(offset: number, length: number): Uint8Array<ArrayBuffer>;
  close(): void;
}

export interface PiecesText { raw: string; aviso: string; provider?: string }
/** Pedaços entregues ao parar. `abort` cancela os que ainda estão em voo. */
export interface PiecesHandle { result: Promise<PiecesText>; abort(): void }

export interface LivePieces {
  /** Um quadro do medidor: `tsMs` desde o início da gravação, `level` RMS linear 0..1. */
  feed(tsMs: number, level: number): void;
  /** Desiste dos pedaços (cancelar, segundo plano, falha do gravador); o fim volta ao caminho de hoje. */
  abandon(code: string): void;
  /** Ao parar, com o arquivo completo. `null` = caminho de hoje: nenhum corte, ou os pedaços já desistiram. */
  finish(): PiecesHandle | null;
}

type Transcribe = typeof transcribePiece;
type Reply = Awaited<ReturnType<Transcribe>>;

const text4 = (bytes: Uint8Array, at: number) => String.fromCharCode(bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]);
const u32 = (bytes: Uint8Array, at: number) => (bytes[at] | (bytes[at + 1] << 8) | (bytes[at + 2] << 16) | (bytes[at + 3] << 24)) >>> 0;

/** Onde começam os dados do WAV. Durante a gravação os tamanhos de RIFF e `data` não são finais: só os dos
 *  pedaços anteriores a `data` contam. `null` = cabeçalho ainda incompleto ou não é WAV. */
export function wavDataOffset(head: Uint8Array): number | null {
  if (head.length < 12 || text4(head, 0) !== 'RIFF' || text4(head, 8) !== 'WAVE') return null;
  let at = 12;
  while (at + 8 <= head.length) {
    if (text4(head, at) === 'data') return at + 8;
    const size = u32(head, at + 4);
    at += 8 + size + (size & 1);
  }
  return null;
}

/** PCM16 16 kHz mono num WAV de 44 bytes. */
export function pieceWav(pcm: Uint8Array): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(44 + pcm.length);
  const view = new DataView(out.buffer);
  const ascii = (at: number, s: string) => { for (let i = 0; i < 4; i++) out[at + i] = s.charCodeAt(i); };
  ascii(0, 'RIFF'); view.setUint32(4, 36 + pcm.length, true); ascii(8, 'WAVE');
  ascii(12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, 16000, true); view.setUint32(28, 32000, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  ascii(36, 'data'); view.setUint32(40, pcm.length, true);
  out.set(pcm, 44);
  return out;
}

interface Slot { piece: DictationPiece; toEnd: boolean; controller?: AbortController; reply?: Promise<Reply> }

export function startLivePieces(opts: {
  server: Server;
  open: () => GrowingFile;
  transcribe?: Transcribe;
  now?: () => number;
  stallMs?: number;
  /** Diário: por que os pedaços desistiram. */
  onFail?: (code: string, detail?: string) => void;
}): LivePieces {
  const transcribe = opts.transcribe ?? transcribePiece;
  const now = opts.now ?? Date.now;
  const stallMs = opts.stallMs ?? STALL_MS;
  const cutter = newDictationCutter();
  const run = new AbortController();
  const slots: Slot[] = [];
  let state: 'live' | 'failed' | 'done' = 'live';
  let file: GrowingFile | null = null;
  let dataAt: number | null = null;
  let waitingSince: number | null = null;

  const close = () => {
    try { file?.close(); } catch { /* só a leitura; o gravador segue com o arquivo */ }
    file = null;
  };
  const fail = (code: string, detail?: string) => {
    if (state !== 'live') return;
    state = 'failed';
    run.abort();
    close();
    opts.onFail?.(code, detail);
  };

  // Lê e manda o pedaço se o arquivo já tem os dados dele. `toEnd`: o resto vai até o fim do arquivo.
  const send = (slot: Slot): boolean => {
    file ??= opts.open();
    const size = file.size();
    if (dataAt === null) dataAt = wavDataOffset(file.read(0, Math.min(size, HEADER_READ)));
    if (dataAt === null) return false;
    const start = dataAt + slot.piece.startMs * PCM_BYTES_PER_MS;
    const end = slot.toEnd ? size : dataAt + slot.piece.endMs * PCM_BYTES_PER_MS;
    if (size < end) return false;
    const pcm = file.read(start, Math.max(0, end - start) & ~1);
    const controller = new AbortController();
    run.signal.addEventListener('abort', () => controller.abort(), { once: true });
    slot.controller = controller;
    slot.reply = transcribe(opts.server, pieceWav(pcm), controller.signal);
    slot.reply.catch((e: unknown) => {
      if (!controller.signal.aborted) fail('piece_failed', e instanceof Error ? e.message : undefined);
    });
    return true;
  };

  // Na ordem: um pedaço sem dados segura os seguintes até o arquivo crescer.
  const flush = () => {
    try {
      for (const slot of slots) {
        if (slot.piece.skip || slot.reply) continue;
        if (send(slot)) {
          // A espera do próximo conta de agora, não de quando o anterior começou a esperar.
          waitingSince = null;
          continue;
        }
        waitingSince ??= now();
        if (now() - waitingSince > stallMs) fail('stalled');
        return;
      }
      waitingSince = null;
    } catch (e) {
      fail('read_failed', e instanceof Error ? e.message : undefined);
    }
  };

  let lastTs = 0;
  return {
    feed(tsMs, level) {
      if (state !== 'live') return;
      // O medidor repete o quadro quando o gravador não andou: não é tempo novo.
      // Roda dentro do laço do medidor: erro aqui desiste dos pedaços, nunca derruba a gravação.
      try {
        if (tsMs > lastTs) {
          lastTs = tsMs;
          const piece = feedDictationFrame(cutter, { ts_ms: tsMs, level });
          if (piece) slots.push({ piece, toEnd: false });
        }
      } catch (e) {
        fail('feed_failed', e instanceof Error ? e.message : undefined);
        return;
      }
      if (slots.length) flush();
    },

    abandon(code) {
      fail(code);
    },

    finish() {
      if (state !== 'live') return null;
      const stop = stopDictationCutter(cutter);
      if (stop.tail === 'whole') {
        state = 'done';
        close();
        return null;
      }
      if (stop.tail === 'merge') slots.pop()?.controller?.abort();
      slots.push({ piece: stop.piece, toEnd: true });
      // Gravação parada: o arquivo está completo e todo pedaço que esperava dados sai agora.
      try {
        for (const slot of slots) {
          if (!slot.piece.skip && !slot.reply && !send(slot)) throw new Error('wav_incomplete');
        }
      } catch (e) {
        fail('read_failed', e instanceof Error ? e.message : undefined);
        return null;
      }
      state = 'done';
      close();
      const replies = slots.flatMap((slot) => slot.reply ? [slot.reply] : []);
      const result = (async (): Promise<PiecesText> => {
        const texts: string[] = [];
        const avisos: string[] = [];
        let provider: string | undefined;
        for (const reply of replies) {
          const r = await reply;
          texts.push(r.raw ?? '');
          if (r.provider) provider = r.provider;
          if (r.aviso && !avisos.includes(r.aviso)) avisos.push(r.aviso);
        }
        return { raw: joinDictationPieces(texts), aviso: avisos.join(' · '), ...(provider ? { provider } : {}) };
      })();
      // Quem recebe decide; sem isto, um resultado ignorado (cancelar) vira rejeição solta.
      result.catch(() => {});
      return { result, abort: () => run.abort() };
    },
  };
}
