import { describe, expect, it, vi } from 'vitest';
import type { Server } from '@hangar/core';
import { PCM_BYTES_PER_MS, STALL_MS, pieceWav, startLivePieces, wavDataOffset, type GrowingFile } from './livePieces';

const server: Server = { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 't' };
const ascii = (text: string) => [...text].map((c) => c.charCodeAt(0));
const u32 = (n: number) => [n & 255, (n >> 8) & 255, (n >> 16) & 255, (n >> 24) & 255];

/** Cabeçalho como o do iPhone (medido): `fmt `, enchimento `FLLR` e `data` com tamanho ainda não final. */
function iphoneHeader(dataAt = 4096): number[] {
  const fmt = [...ascii('fmt '), ...u32(16), 1, 0, 1, 0, ...u32(16000), ...u32(32000), 2, 0, 16, 0];
  const fill = dataAt - 12 - fmt.length - 8 - 8;
  return [...ascii('RIFF'), ...u32(0), ...ascii('WAVE'), ...fmt, ...ascii('FLLR'), ...u32(fill), ...new Array(fill).fill(0),
    ...ascii('data'), ...u32(0)];
}

/** WAV que cresce como o do gravador: o PCM chega até `writtenMs`; cada ms leva o byte `ms % 251`. */
function growingWav(dataAt = 4096) {
  const header = iphoneHeader(dataAt);
  let writtenMs = 0;
  const bytes = () => {
    const out = new Uint8Array(header.length + writtenMs * PCM_BYTES_PER_MS);
    out.set(header);
    for (let ms = 0; ms < writtenMs; ms++) out.fill(ms % 251, header.length + ms * PCM_BYTES_PER_MS, header.length + (ms + 1) * PCM_BYTES_PER_MS);
    return out;
  };
  const close = vi.fn();
  const file: GrowingFile = {
    size: () => header.length + writtenMs * PCM_BYTES_PER_MS,
    read: (offset, length) => bytes().slice(offset, offset + length),
    close,
  };
  return { file, close, write: (ms: number) => { writtenMs = Math.max(writtenMs, ms); } };
}

type Reply = { raw: string; provider?: string; aviso?: string | null };
function api() {
  const calls: { wav: Uint8Array; signal: AbortSignal; resolve: (r: Reply) => void; reject: (e: Error) => void }[] = [];
  const transcribe = vi.fn((_server: Server, wav: Blob | Uint8Array<ArrayBuffer>, signal?: AbortSignal) =>
    new Promise<Reply>((resolve, reject) => { calls.push({ wav: wav as Uint8Array, signal: signal!, resolve, reject }); }));
  return { calls, transcribe };
}

/** Quadros de 55 ms, como o medidor ao vivo: fala alterna 0.06/0.04, pausa 0.002. */
function speak(run: ReturnType<typeof startLivePieces>, wav: ReturnType<typeof growingWav>, from: number,
  segments: { ms: number; voice: boolean }[], lagMs = 0, clock?: { now: number }) {
  let ts = from;
  let start = from;
  for (const seg of segments) {
    const end = start + seg.ms;
    for (let i = 0; ts + 55 <= end; i++) {
      ts += 55;
      if (clock) clock.now = ts;
      wav.write(ts - lagMs);
      run.feed(ts, seg.voice ? (i % 2 ? 0.04 : 0.06) : 0.002);
    }
    start = end;
  }
  return ts;
}

/** PCM de um pedaço enviado, conferido pelo byte de cada ms (`ms % 251`). */
function pcmSpan(wav: Uint8Array): { firstMs: number; ms: number } {
  expect(String.fromCharCode(...wav.slice(0, 4))).toBe('RIFF');
  expect(String.fromCharCode(...wav.slice(36, 40))).toBe('data');
  const size = wav[40] | (wav[41] << 8) | (wav[42] << 16) | (wav[43] << 24);
  expect(size).toBe(wav.length - 44);
  return { firstMs: wav[44], ms: size / PCM_BYTES_PER_MS };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('wavDataOffset', () => {
  it('acha o `data` depois do enchimento do iPhone (byte 4096), sem confiar nos tamanhos ainda não finais', () => {
    expect(wavDataOffset(new Uint8Array(iphoneHeader(4096)))).toBe(4096);
    expect(wavDataOffset(new Uint8Array(iphoneHeader(44 + 8 + 8)))).toBe(44 + 8 + 8);
  });

  it('cabeçalho ainda incompleto ou que não é WAV: sem posição', () => {
    expect(wavDataOffset(new Uint8Array(iphoneHeader(4096)).slice(0, 2000))).toBeNull();
    expect(wavDataOffset(new Uint8Array([...ascii('OggS'), 0, 0, 0, 0, ...ascii('WAVE')]))).toBeNull();
  });
});

describe('pieceWav', () => {
  it('embrulha PCM16 16 kHz mono num WAV de 44 bytes', () => {
    const wav = pieceWav(new Uint8Array([1, 2, 3, 4]));
    expect(wav.length).toBe(48);
    expect(wavDataOffset(wav)).toBe(44);
    expect([...wav.slice(22, 28)]).toEqual([1, 0, ...u32(16000)]);
    expect([...wav.slice(44)]).toEqual([1, 2, 3, 4]);
  });
});

describe('startLivePieces', () => {
  it('ditado de 6 s: nenhum pedaço sai e o fim segue o caminho de hoje', () => {
    const { transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    speak(run, wav, 0, [{ ms: 3000, voice: true }, { ms: 150, voice: false }, { ms: 2850, voice: true }]);
    expect(run.finish()).toBeNull();
    expect(transcribe).not.toHaveBeenCalled();
  });

  it('16 s com pausa aos 9 s: um pedaço durante a fala, o resto ao parar, cru juntado na ordem', async () => {
    const { calls, transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    const end = speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 6400, voice: true }]);
    expect(calls).toHaveLength(1);
    // Corte no meio da pausa (caso de 55 ms das fixtures): 9185 ms de áudio desde o começo.
    expect(pcmSpan(calls[0].wav)).toEqual({ firstMs: 0, ms: 9185 });
    wav.write(end + 120);   // o que o gravador escreveu depois do último quadro também é do resto
    const handle = run.finish()!;
    expect(calls).toHaveLength(2);
    expect(pcmSpan(calls[1].wav)).toEqual({ firstMs: 9185 % 251, ms: end + 120 - 9185 });
    calls[1].resolve({ raw: ' Não sei o que fazer ', provider: 'Groq' });
    calls[0].resolve({ raw: 'ela trava', provider: 'Groq', aviso: 'reserva usada' });
    await expect(handle.result).resolves.toEqual({ raw: 'ela trava. Não sei o que fazer', aviso: 'reserva usada', provider: 'Groq' });
  });

  it('final curto: o último pedaço é cancelado e reenviado junto com o resto', async () => {
    const { calls, transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    const end = speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 1700, voice: true }]);
    expect(calls).toHaveLength(1);
    const handle = run.finish()!;
    expect(calls[0].signal.aborted).toBe(true);
    expect(calls).toHaveLength(2);
    expect(pcmSpan(calls[1].wav)).toEqual({ firstMs: 0, ms: end });
    calls[0].reject(new DOMException('aborted', 'AbortError'));
    calls[1].resolve({ raw: 'tudo junto' });
    await expect(handle.result).resolves.toMatchObject({ raw: 'tudo junto', aviso: '' });
  });

  it('pedaço que falha durante a fala desiste dos pedaços: os outros são abortados e o fim é o caminho de hoje', async () => {
    const { calls, transcribe } = api();
    const onFail = vi.fn();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe, onFail });
    // Dois cortes: aos ~9,2 s e aos ~18,4 s.
    speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 8600, voice: true }, { ms: 600, voice: false },
      { ms: 3000, voice: true }]);
    expect(calls).toHaveLength(2);
    calls[1].reject(new Error('503: provedor fora'));
    await flush();
    expect(onFail).toHaveBeenCalledExactlyOnceWith('piece_failed', '503: provedor fora');
    expect(calls[0].signal.aborted).toBe(true);
    expect(run.finish()).toBeNull();
    expect(calls).toHaveLength(2);
  });

  it('pedaço do fim que falha depois de parar rejeita o resultado', async () => {
    const { calls, transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 6400, voice: true }]);
    const handle = run.finish()!;
    calls[0].resolve({ raw: 'um' });
    calls[1].reject(new Error('502: groq'));
    await expect(handle.result).rejects.toThrow('502: groq');
  });

  it('abandonar (cancelar, segundo plano) aborta o que está em voo e o fim volta ao caminho de hoje', () => {
    const { calls, transcribe } = api();
    const onFail = vi.fn();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe, onFail });
    speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 2000, voice: true }]);
    run.abandon('background');
    expect(calls[0].signal.aborted).toBe(true);
    expect(onFail).toHaveBeenCalledExactlyOnceWith('background', undefined);
    expect(run.finish()).toBeNull();
    expect(wav.close).toHaveBeenCalled();
  });

  it('abortar o resultado já entregue cancela os pedaços em voo', () => {
    const { calls, transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 6400, voice: true }]);
    const handle = run.finish()!;
    handle.abort();
    expect(calls.every((c) => c.signal.aborted)).toBe(true);
  });

  it('resposta atrasada de uma gravação anterior não entra no texto da nova', async () => {
    const old = api();
    const wavA = growingWav();
    const runA = startLivePieces({ server, open: () => wavA.file, transcribe: old.transcribe });
    speak(runA, wavA, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 2000, voice: true }]);
    runA.abandon('cancelled');
    const fresh = api();
    const wavB = growingWav();
    const runB = startLivePieces({ server, open: () => wavB.file, transcribe: fresh.transcribe });
    speak(runB, wavB, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 6400, voice: true }]);
    const handle = runB.finish()!;
    old.calls[0].resolve({ raw: 'velho' });
    fresh.calls[0].resolve({ raw: 'novo' });
    fresh.calls[1].resolve({ raw: 'fim' });
    await expect(handle.result).resolves.toMatchObject({ raw: 'novo fim' });
    expect(runA.finish()).toBeNull();
  });

  it('pedaço cortado antes de o arquivo ter os dados espera o próximo quadro', () => {
    const { calls, transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    // O gravador escreve com até ~530 ms de atraso.
    speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }], 530);
    expect(calls).toHaveLength(0);
    speak(run, wav, 9570, [{ ms: 300, voice: true }], 530);
    expect(calls).toHaveLength(1);
    expect(pcmSpan(calls[0].wav)).toEqual({ firstMs: 0, ms: 9185 });
  });

  it('final curto com o último pedaço ainda esperando dados: sai um só, até onde o arquivo completo chega', async () => {
    const { calls, transcribe } = api();
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe });
    // O relógio do gravador anda à frente dos bytes: o arquivo final para em 9100 ms.
    speak(run, wav, 0, [{ ms: 9000, voice: true }, { ms: 600, voice: false }, { ms: 300, voice: true }], 800);
    wav.write(9100);
    expect(calls).toHaveLength(0);
    const handle = run.finish()!;
    expect(calls).toHaveLength(1);
    expect(pcmSpan(calls[0].wav)).toEqual({ firstMs: 0, ms: 9100 });
    calls[0].resolve({ raw: 'tudo' });
    await expect(handle.result).resolves.toMatchObject({ raw: 'tudo' });
  });

  it('dados que param de chegar viram falha, não silêncio', () => {
    const { calls, transcribe } = api();
    const onFail = vi.fn();
    const clock = { now: 0 };
    const wav = growingWav();
    const run = startLivePieces({ server, open: () => wav.file, transcribe, onFail, now: () => clock.now });
    speak(run, wav, 0, [{ ms: 8000, voice: true }], 0, clock);
    // A partir daqui o arquivo não cresce mais.
    let ts = 8000;
    const stuck = { write: () => {}, file: wav.file, close: wav.close };
    ts = speak(run, stuck as unknown as ReturnType<typeof growingWav>, ts, [{ ms: 1000, voice: true }, { ms: 600, voice: false }], 0, clock);
    expect(onFail).not.toHaveBeenCalled();
    speak(run, stuck as unknown as ReturnType<typeof growingWav>, ts, [{ ms: STALL_MS + 200, voice: true }], 0, clock);
    expect(onFail).toHaveBeenCalledExactlyOnceWith('stalled', undefined);
    expect(calls).toHaveLength(0);
    expect(run.finish()).toBeNull();
  });
});
