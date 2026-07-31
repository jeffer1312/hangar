// Relogio do player de TTS. Separado do componente porque `duration` de um <audio> chega NaN antes
// dos metadados e Infinity em stream sem tamanho — os dois pintariam "NaN:aN" na barra.
export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
  const total = Math.floor(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}
