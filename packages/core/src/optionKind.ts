// Porte de OptionButtons.svelte:14-25. Heurística textual (en/pt) do menu de permissão do Claude
// Code; não casou → lista genérica. ponytail: se o texto do menu mudar, degrada pro genérico.
export type OptionKind = 'allow' | 'always' | 'deny' | 'other';
export function kindOf(o: string): OptionKind {
  const l = o.toLowerCase();
  if (/don'?t ask again|always|sempre|n[aã]o perguntar/.test(l)) return 'always';
  if (/^(yes|sim)\b/.test(l)) return 'allow';
  if (/^(no|n[aã]o)\b/.test(l)) return 'deny';
  return 'other';
}
export function isPermission(options: string[]): boolean {
  const kinds = options.map(kindOf);
  return options.length >= 2 && options.length <= 4 && kinds.includes('allow') && kinds.includes('deny');
}
