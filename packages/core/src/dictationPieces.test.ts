import { describe, expect, it } from 'vitest';
import fixtures from './dictationPieces.fixtures.json';
import {
  FORCED_WINDOW_MS, FRAME_MS, MAX_PIECE_MS, MIN_PIECE_MS, MIN_VOICE_MS, PAUSE_MS, TAIL_MERGE_MS,
  feedDictationFrame, newDictationCutter, stopDictationCutter, type DictationPiece, type LevelFrame,
} from './dictationPieces';

type Expected = { startMs: number; endMs: number; voicedMs: number[]; skip: boolean };

function frames(c: (typeof fixtures.cases)[number]): LevelFrame[] {
  const out: LevelFrame[] = [];
  let start = 0;
  let ts = c.frameMs;
  for (const seg of c.segments) {
    const end = start + seg.ms;
    for (let i = 0; ts <= end; i++, ts += c.frameMs) {
      out.push({ ts_ms: ts, level: seg.levels[i % seg.levels.length] });
    }
    start = end;
  }
  return out;
}

function expectPiece(got: DictationPiece, want: Expected) {
  expect({ startMs: got.startMs, endMs: got.endMs, skip: got.skip }).toEqual({
    startMs: want.startMs, endMs: want.endMs, skip: want.skip,
  });
  expect(got.voicedMs).toBeGreaterThanOrEqual(want.voicedMs[0]);
  expect(got.voicedMs).toBeLessThanOrEqual(want.voicedMs[1]);
}

describe('dictationPieces: casos compartilhados', () => {
  it('as constantes são as do contrato', () => {
    expect({
      frameMs: FRAME_MS, minPieceMs: MIN_PIECE_MS, pauseMs: PAUSE_MS, maxPieceMs: MAX_PIECE_MS,
      forcedWindowMs: FORCED_WINDOW_MS, tailMergeMs: TAIL_MERGE_MS, minVoiceMs: MIN_VOICE_MS,
    }).toEqual(fixtures.constants);
  });

  for (const c of fixtures.cases) {
    it(c.name, () => {
      const cutter = newDictationCutter();
      const pieces: DictationPiece[] = [];
      for (const f of frames(c)) {
        const piece = feedDictationFrame(cutter, f);
        if (piece) pieces.push(piece);
      }
      expect(pieces.length).toBe(c.pieces.length);
      pieces.forEach((p, i) => expectPiece(p, c.pieces[i]));
      const stop = stopDictationCutter(cutter);
      expect(stop.tail).toBe(c.stop.tail);
      expectPiece(stop.piece, c.stop.piece);
    });
  }
});
