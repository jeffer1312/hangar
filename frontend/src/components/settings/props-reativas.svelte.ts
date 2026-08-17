// Props reativas para teste: trocar o alvo com o componente MONTADO.
export function criarProps<T extends object>(inicial: T): T {
  const p = $state(inicial);
  return p;
}
