// Decide onde cortar o ditado em pedaços enquanto a pessoa ainda fala, para transcrever cada pedaço
// em paralelo. Puro e sem relógio próprio: o tempo entra no quadro. O desktop nativo reimplementa a
// mesma regra e passa pelos mesmos casos (dictationPieces.fixtures.json), por isso nada aqui depende
// de recurso do JS. MUTA o estado que recebe.
//
// Voz × silêncio é o detector do encerramento automático (vad.ts), sem mudar nada nele: um quadro é
// voz quando o detector está armado e não está em silêncio.

import { PISO_ABSOLUTO, novoEstadoVad, passoVad, type EstadoVad } from './vad';

/** Quadro nominal dos casos compartilhados; ao vivo os quadros chegam a cada ~55 ms e a regra segue o ts_ms. */
export const FRAME_MS = 30;
/** Pedaço curto demais piora a transcrição (mais trecho inventado); abaixo disso nem corta. */
export const MIN_PIECE_MS = 8000;
/** Pausa que já separa frases; entre palavras a pausa fica bem abaixo disso. */
export const PAUSE_MS = 400;
/** Sem pausa, corta mesmo assim para o último pedaço não crescer sem fim. */
export const MAX_PIECE_MS = 20000;
/** Onde procurar o quadro mais baixo no corte forçado. */
export const FORCED_WINDOW_MS = 3000;
/** Resto mais curto que isto vai junto com o último pedaço: sozinho perde o contexto da frase. */
export const TAIL_MERGE_MS = 4000;
/** Pedaço com menos voz que isto não é enviado: é onde o modelo inventa texto. */
export const MIN_VOICE_MS = 500;

/** ts_ms = fim do quadro, contado do início da gravação; level = RMS linear 0..1. */
export interface LevelFrame { ts_ms: number; level: number }
export interface DictationPiece { startMs: number; endMs: number; voicedMs: number; skip: boolean }
/** whole = nenhum corte: o ditado segue o caminho de hoje, arquivo inteiro.
 *  merge = o piece substitui o último pedaço já cortado (último + resto). */
export interface DictationStop { tail: 'whole' | 'send' | 'merge'; piece: DictationPiece }

interface Frame { ts: number; level: number; dur: number; voiced: boolean }

export interface DictationCutter {
  vad: EstadoVad;
  lastTs: number;
  pieceStart: number;
  /** Quadros do pedaço em curso (no máximo ~20 s). */
  frames: Frame[];
  /** Fim do último quadro de voz antes do silêncio em curso, ou null se o último quadro foi voz. */
  pauseStart: number | null;
  lastPiece: DictationPiece | null;
}

export function newDictationCutter(): DictationCutter {
  return { vad: novoEstadoVad(), lastTs: 0, pieceStart: 0, frames: [], pauseStart: null, lastPiece: null };
}

function cutAt(c: DictationCutter, at: number): DictationPiece {
  let voicedMs = 0;
  while (c.frames.length && c.frames[0].ts <= at) {
    const f = c.frames.shift()!;
    if (f.voiced) voicedMs += f.dur;
  }
  const piece = { startMs: c.pieceStart, endMs: at, voicedMs, skip: voicedMs < MIN_VOICE_MS };
  c.pieceStart = at;
  c.lastPiece = piece;
  return piece;
}

export function feedDictationFrame(c: DictationCutter, frame: LevelFrame): DictationPiece | null {
  const ts = frame.ts_ms;
  const dur = Math.max(0, ts - c.lastTs);
  passoVad(c.vad, frame.level, ts);
  // passoVad zera silencioDesde exatamente quando o quadro não é silêncio. O piso vale para o quadro,
  // não para o pico: o pico sobe devagar e, em quem fala baixo, mal passa do piso, e o pedaço de fala
  // real seria descartado.
  const voiced = frame.level > PISO_ABSOLUTO && c.vad.silencioDesde === null;
  c.frames.push({ ts, level: frame.level, dur, voiced });
  if (voiced) c.pauseStart = null;
  else if (c.pauseStart === null) c.pauseStart = c.lastTs;
  c.lastTs = ts;

  if (c.pauseStart !== null) {
    // Pausa que atravessa um corte conta a partir do corte: ela pertence ao pedaço novo.
    const start = Math.max(c.pauseStart, c.pieceStart);
    const middle = start + Math.round((ts - start) / 2);
    if (ts - start >= PAUSE_MS && middle - c.pieceStart >= MIN_PIECE_MS) return cutAt(c, middle);
  }
  if (ts - c.pieceStart >= MAX_PIECE_MS) {
    // ponytail: varredura linear da janela (~100 quadros); empate fica com o mais antigo.
    let low: Frame | null = null;
    for (const f of c.frames) {
      if (f.ts >= ts - FORCED_WINDOW_MS && (low === null || f.level < low.level)) low = f;
    }
    return cutAt(c, low!.ts);
  }
  return null;
}

export function stopDictationCutter(c: DictationCutter): DictationStop {
  const last = c.lastPiece;
  const rest = cutAt(c, c.lastTs);
  if (!last) return { tail: 'whole', piece: rest };
  if (rest.endMs - rest.startMs >= TAIL_MERGE_MS) return { tail: 'send', piece: rest };
  const voicedMs = last.voicedMs + rest.voicedMs;
  return { tail: 'merge', piece: { startMs: last.startMs, endMs: rest.endMs, voicedMs, skip: voicedMs < MIN_VOICE_MS } };
}
