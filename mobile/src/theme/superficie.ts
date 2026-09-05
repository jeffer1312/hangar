interface ComGlass {
  tokens: { glass: { rgb: readonly number[] } };
  surfaceAlpha: number;
}

// Caixa DENTRO de um painel (card, bolha, chip, campo). Nunca `bg.elevated`/`bg.base` cru: cor
// chapada não acompanha o slider de Solidez e vira retângulo opaco boiando sobre o papel de parede.
// k é o degrau: 0.6 no lugar do antigo `bg.surface`, 0.8 no lugar do `bg.elevated`.
export const superficie = (t: ComGlass, k = 0.6) =>
  `rgba(${t.tokens.glass.rgb.join(',')},${t.surfaceAlpha * k})`;
