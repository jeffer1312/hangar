import * as m from '../../paraglide/messages';

// Nomes dos modos como o app de PC escreve. Modo que o app não conhece (CLI novo) sai como o id cru:
// dado, não interface.
const LABEL: Record<string, () => string> = {
  plan: m.native_mode_plan,
  auto: m.native_mode_auto,
  manual: m.native_mode_manual,
  acceptEdits: m.native_mode_acceptEdits,
  bypassPermissions: m.native_mode_bypassPermissions,
  dontAsk: m.native_mode_dontAsk,
};

const KIMI_LABEL: Record<string, () => string> = {
  manual: m.native_kimi_mode_manual,
  yolo: m.native_kimi_mode_yolo,
  auto: m.native_kimi_mode_auto,
};

export const permissionLabel = (mode: string, provider?: string) =>
  (provider === 'kimi' ? KIMI_LABEL : LABEL)[mode]?.() ?? mode;
