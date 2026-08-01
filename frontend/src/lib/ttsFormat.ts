// Relogio do player de TTS. Separado do componente porque `duration` de um <audio> chega NaN antes
// dos metadados e Infinity em stream sem tamanho — os dois pintariam "NaN:aN" na barra.
export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
  const total = Math.floor(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

const TERMINADOR_DE_FRASE = /[.!?]/;

/**
 * Corta um texto em ATÉ `limite` caracteres, terminando na última frase (`.`/`!`/`?`) ou, na falta
 * dela, na última palavra inteira que couber — nunca no meio de uma palavra. Usada pela amostra de
 * voz (Feature A): a amostra é o próprio último trecho ouvido, cortado pra não gastar demais.
 */
export function cortarAmostra(texto: string, limite = 200): string {
  const t = texto.trim();
  if (t.length <= limite) return t;
  const fatia = t.slice(0, limite);
  let corte = -1;
  for (let i = fatia.length - 1; i >= 0; i--) {
    if (TERMINADOR_DE_FRASE.test(fatia[i])) { corte = i + 1; break; }
  }
  if (corte === -1) {
    const espaco = fatia.lastIndexOf(' ');
    // ponytail: sem frase e sem espaço (uma palavra só maior que o limite) não há fronteira
    // possível — cai pro corte duro em `limite`, único caso em que a regra cede.
    corte = espaco > 0 ? espaco : fatia.length;
  }
  return fatia.slice(0, corte).trim();
}
