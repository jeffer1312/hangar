import { LOOP_GUIDE as LOOP_GUIDE_KEYS, type LoopGuideMessageKey } from '@hangar/core';
import * as m from '../paraglide/messages';

export interface LoopGuideSection {
  title: string;
  body: string;
}

const messages: Record<LoopGuideMessageKey, () => string> = {
  loop_objetivo_titulo: m.loop_objetivo_titulo,
  loop_objetivo_corpo: m.loop_objetivo_corpo,
  loop_check_titulo: m.loop_check_titulo,
  loop_check_corpo: m.loop_check_corpo,
  loop_iteracoes_titulo: m.loop_iteracoes_titulo,
  loop_iteracoes_corpo: m.loop_iteracoes_corpo,
  loop_sinais_titulo: m.loop_sinais_titulo,
  loop_sinais_corpo: m.loop_sinais_corpo,
  loop_dica_titulo: m.loop_dica_titulo,
  loop_dica_corpo: m.loop_dica_corpo,
};

export const LOOP_GUIDE: LoopGuideSection[] = LOOP_GUIDE_KEYS.map((section) => ({
  title: messages[section.title](),
  body: messages[section.body](),
}));
