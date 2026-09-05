import type { ComponentType } from 'react';
import * as Lucide from 'lucide-react-native';
import { useUnistyles } from 'react-native-unistyles';

export type IconName = keyof typeof Lucide;

export function Icon({ name, size = 18, color }: { name: IconName; size?: number; color?: string }) {
  const { theme } = useUnistyles();
  const Cmp = Lucide[name] as ComponentType<{ size: number; color: string; strokeWidth?: number }>;
  return <Cmp size={size} color={color ?? theme.tokens.text.secondary} strokeWidth={1.75} />;
}
