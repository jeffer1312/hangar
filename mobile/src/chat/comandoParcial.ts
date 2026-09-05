// O texto do composer ainda é só o NOME de um comando sendo digitado? Devolve o filtro (o que
// veio depois da barra) ou null.
//
// A barra sozinha não basta: caminho absoluto colado (`/home/x/y.py`) começa igual, e abrir a
// lista de comandos por cima dele é atrapalhar quem colou um arquivo. Nome de comando é uma
// palavra — letra, dígito, `_` ou `-` —, sem segunda barra, sem espaço e sem quebra de linha.
export function comandoParcial(texto: string): string | null {
  if (!texto.startsWith('/')) return null;
  const resto = texto.slice(1);
  return /^[\w-]*$/.test(resto) ? resto : null;
}
