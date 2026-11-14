import type { ComponentType } from 'react';
import { Platform } from 'react-native';
import * as Lucide from 'lucide-react-native';
import { SymbolView } from 'expo-symbols';
import { useUnistyles } from 'react-native-unistyles';
import { sfSymbol } from './sfSymbols';

// Exclui os membros do namespace que não são ícones (helpers/contexto do pacote).
const NOT_ICONS = ['Icon', 'createLucideIcon', 'useLucideContext', 'LucideProvider'] as const;
export type IconName = Exclude<keyof typeof Lucide, typeof NOT_ICONS[number]>;
export const isIconName = (name: string): name is IconName => name in Lucide && !(NOT_ICONS as readonly string[]).includes(name);

type IconCmp = ComponentType<{ size: number; color: string; strokeWidth?: number }>;

// O Lucide escala o traço com o tamanho: fixo em 1.75 o ícone de 11 some e o de 22 engrossa.
// Mira ~1.6px na tela em qualquer tamanho.
const traco = (size: number) => Math.max(1.5, Math.min(2.5, (1.6 * 24) / size));

// iOS: SF Symbol do sistema quando há equivalente; Android e nome sem símbolo ficam no Lucide.
export function Icon({ name, size = 18, color }: { name: IconName; size?: number; color?: string }) {
  const { theme } = useUnistyles();
  const cor = color ?? theme.tokens.text.secondary;
  const Cmp = Lucide[name] as IconCmp | undefined;
  if (!Cmp) {
    console.warn('Icon: nome desconhecido', name);
    return <Lucide.CircleAlert size={size} color={cor} strokeWidth={1.75} />;
  }
  const lucide = <Cmp size={size} color={cor} strokeWidth={traco(size)} />;
  const symbol = sfSymbol(name, Platform.OS);
  if (!symbol) return lucide;
  return <SymbolView name={symbol} size={size} tintColor={cor} weight="regular" fallback={lucide} />;
}
