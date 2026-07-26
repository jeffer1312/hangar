import { describe, it, expect } from 'vitest';
import { targetSize, precisaPreparo, temMarcador, MAX_EDGE } from './imagePrep';

describe('targetSize', () => {
  it('não mexe no que já cabe', () => {
    expect(targetSize(1568, 900)).toBeNull();
    expect(targetSize(800, 600)).toBeNull();
    expect(targetSize(1, 1)).toBeNull();
  });

  it('encolhe pelo lado maior, mantendo a proporção', () => {
    // A foto real que motivou isto: 4032x3024, 2554 KB.
    expect(targetSize(4032, 3024)).toEqual({ w: 1568, h: 1176 });
    // Retrato (print de celular) — o lado maior é a altura.
    expect(targetSize(1179, 2556)).toEqual({ w: 723, h: 1568 });
  });

  it('nunca gera lado 0 em imagem muito alongada', () => {
    const r = targetSize(4000, 3)!;
    expect(r.w).toBe(MAX_EDGE);
    expect(r.h).toBeGreaterThanOrEqual(1);
  });

  it('ignora dimensão inválida', () => {
    expect(targetSize(0, 100)).toBeNull();
    expect(targetSize(NaN, 100)).toBeNull();
  });
});

describe('precisaPreparo', () => {
  it('formato que o app não abre passa pelo canvas mesmo cabendo no tamanho', () => {
    expect(precisaPreparo('image/heic', 800, 600)).toBe(true);
  });

  it('formato aceito e dentro do tamanho não precisa de nada', () => {
    expect(precisaPreparo('image/jpeg', 800, 600)).toBe(false);
    expect(precisaPreparo('image/png', 1568, 1000)).toBe(false);
  });

  it('formato aceito mas grande demais precisa', () => {
    expect(precisaPreparo('image/jpeg', 4032, 3024)).toBe(true);
  });
});

describe('temMarcador (detecção de animação)', () => {
  const bytesDe = (s: string) => new Uint8Array([...s].map((c) => c.charCodeAt(0)));

  it('acha o ANIM de um WebP animado', () => {
    expect(temMarcador(bytesDe('RIFF....WEBPVP8X...ANIM...'), 'ANIM')).toBe(true);
  });

  it('não acha em WebP estático', () => {
    expect(temMarcador(bytesDe('RIFF....WEBPVP8 ....'), 'ANIM')).toBe(false);
  });

  it('acha o acTL de um APNG', () => {
    expect(temMarcador(bytesDe('\x89PNG....IHDR....acTL....IDAT'), 'acTL')).toBe(true);
  });

  it('não confunde marcador cortado no fim do buffer', () => {
    expect(temMarcador(bytesDe('xxxxANI'), 'ANIM')).toBe(false);
  });
});
