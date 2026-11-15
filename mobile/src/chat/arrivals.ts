/**
 * Quantas linhas do fim de `next` chegaram desde `prev`. Só conta acréscimo pequeno depois da última
 * linha que já estava na tela: conversa abrindo, página antiga chegando em cima, troca da última linha
 * ou lote grande valem 0 e aparecem direto.
 */
export function arrivedCount(prev: readonly string[] | null, next: readonly string[]): number {
  if (!prev?.length) return 0;
  const at = next.lastIndexOf(prev[prev.length - 1]!);
  if (at < 0) return 0;
  const added = next.length - 1 - at;
  return added <= 4 ? added : 0;
}
