import { PixelRatio } from 'react-native';

/** Lado da miniatura de mídia na conversa, em pontos. */
export const THUMB_BOX = 96;

// Só o que o servidor reduz: SVG e AVIF voltariam inteiros numa URL a mais, e o GIF perderia o movimento.
const REDUCIBLE = /\.(png|jpe?g|webp|bmp)$/i;

/** Largura em pixels pedida ao servidor (`w`, 16..1024) para uma caixa de `box` pontos. */
export function thumbWidth(box: number, scale: number = PixelRatio.get()): number {
  return Math.min(1024, Math.max(16, Math.round(box * scale)));
}

/** Largura da miniatura do arquivo, ou `undefined` quando o servidor não a reduz. */
export function thumbWidthFor(name: string, box: number = THUMB_BOX): number | undefined {
  return REDUCIBLE.test(name) ? thumbWidth(box) : undefined;
}
