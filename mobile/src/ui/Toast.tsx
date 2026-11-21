import type { PropsWithChildren } from 'react';
import { View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { toast as sonner, Toaster as SonnerToaster, type ToasterProps } from 'sonner-native';
import { haptic } from './haptics';

const MOD_TEXT_MAX = 300;
const MOD_VISIBLE_MAX = 4;
// Avisos de mod na tela, do mais antigo ao mais novo: um mod insistente não empilha a tela inteira.
const modVisible: (string | number)[] = [];

function modGone(id: string | number): void {
  const i = modVisible.indexOf(id);
  if (i >= 0) modVisible.splice(i, 1);
}

export const toast = {
  ok: (msg: string) => { haptic.success(); return sonner.success(msg); },
  erro: (msg: string) => { haptic.error(); return sonner.error(msg); },
  aviso: (msg: string) => sonner(msg, { duration: 12_000 }),
  passive: (msg: string) => sonner(msg, { duration: 3_000, toasterId: 'passive' }),
  /** Fica até `dismiss`; o teto cobre a reabertura mais longa do servidor (12 min). */
  progress: (msg: string) => sonner(msg, { duration: 15 * 60_000 }),
  dismiss: (id: string | number) => sonner.dismiss(id),
  /** Aviso de um mod do Claude Code: o nome do mod é o título, como na caixa do terminal. */
  mod: (msg: string, plugin: string, ms: number) => {
    while (modVisible.length >= MOD_VISIBLE_MAX) sonner.dismiss(modVisible.shift());
    const text = msg.length > MOD_TEXT_MAX ? `${msg.slice(0, MOD_TEXT_MAX)}…` : msg;
    const opts = { duration: ms, onDismiss: modGone, onAutoClose: modGone };
    const id = plugin ? sonner(plugin, { ...opts, description: text }) : sonner(text, opts);
    modVisible.push(id);
    return id;
  },
};
function PassiveToastWrapper({ children }: PropsWithChildren) {
  return <View pointerEvents="none" collapsable={false} style={{ width: '100%' }}>{children}</View>;
}

export function Toaster(props: ToasterProps) {
  const insets = useSafeAreaInsets();
  return <>
    <SonnerToaster {...props} />
    <SonnerToaster id="passive" position="top-center" offset={insets.top + 64}
      visibleToasts={1} enableStacking={false} ToastWrapper={PassiveToastWrapper} />
  </>;
}
