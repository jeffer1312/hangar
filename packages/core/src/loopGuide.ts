// Chaves das seções do guia do loop. Cada cliente traduz no próprio runtime de i18n.

export type LoopGuideMessageKey =
  | 'loop_objetivo_titulo'
  | 'loop_objetivo_corpo'
  | 'loop_check_titulo'
  | 'loop_check_corpo'
  | 'loop_iteracoes_titulo'
  | 'loop_iteracoes_corpo'
  | 'loop_sinais_titulo'
  | 'loop_sinais_corpo'
  | 'loop_dica_titulo'
  | 'loop_dica_corpo';

export interface LoopGuideSection {
  title: LoopGuideMessageKey;
  body: LoopGuideMessageKey;
}

export const LOOP_GUIDE: readonly LoopGuideSection[] = [
  { title: 'loop_objetivo_titulo', body: 'loop_objetivo_corpo' },
  { title: 'loop_check_titulo', body: 'loop_check_corpo' },
  { title: 'loop_iteracoes_titulo', body: 'loop_iteracoes_corpo' },
  { title: 'loop_sinais_titulo', body: 'loop_sinais_corpo' },
  { title: 'loop_dica_titulo', body: 'loop_dica_corpo' },
];
