import type { ComponentType } from 'react';
import * as Lucide from 'lucide-react-native';
import { useUnistyles } from 'react-native-unistyles';

// Exclui os membros do namespace que não são ícones (helpers/contexto do pacote).
export type IconName = Exclude<keyof typeof Lucide, 'Icon' | 'createLucideIcon' | 'useLucideContext' | 'LucideProvider'>;

type IconCmp = ComponentType<{ size: number; color: string; strokeWidth?: number }>;

export function Icon({ name, size = 18, color }: { name: IconName; size?: number; color?: string }) {
  const { theme } = useUnistyles();
  const cor = color ?? theme.tokens.text.secondary;
  const Cmp = Lucide[name] as IconCmp | undefined;
  if (!Cmp) {
    console.warn('Icon: nome desconhecido', name);
    return <Lucide.CircleAlert size={size} color={cor} strokeWidth={1.75} />;
  }
  return <Cmp size={size} color={cor} strokeWidth={1.75} />;
}
