// Guia in-app do loop runner (botão ? no LoopSheet). Texto estático, sem endpoint — VERBATIM da
// seção "Guia in-app" de docs/superpowers/specs/2026-07-22-loop-runner-design.md. Colapsável por
// seção, cabe no sheet mobile.

export interface LoopGuideSection {
  title: string;
  body: string;
}

import * as m from '../paraglide/messages';

export const LOOP_GUIDE: LoopGuideSection[] = [
  {
    title: m.loop_objetivo_titulo(),
    body: m.loop_objetivo_corpo(),
  },
  {
    title: m.loop_check_titulo(),
    body: m.loop_check_corpo(),
  },
  {
    title: m.loop_iteracoes_titulo(),
    body: m.loop_iteracoes_corpo(),
  },
  {
    title: m.loop_sinais_titulo(),
    body: m.loop_sinais_corpo(),
  },
  {
    title: m.loop_dica_titulo(),
    body: m.loop_dica_corpo(),
  },
];
