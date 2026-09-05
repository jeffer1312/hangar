import type { ReactNode } from 'react';
import { MenuView } from '@react-native-menu/menu';
import * as Haptics from 'expo-haptics';
import * as m from '../../paraglide/messages';

interface Props {
  children: ReactNode;
  temCwd: boolean;
  onRenomear: () => void;
  onGit: () => void;
  onLoop: () => void;
  onExcluir: () => void;
}

// O toque longo é do menu nativo (`shouldOpenOnLongPress`), não do Pressable da linha: dois
// donos do mesmo gesto brigariam com o swipe. Por isso o haptic mora aqui.
export function SessionMenu({ children, temCwd, onRenomear, onGit, onLoop, onExcluir }: Props) {
  return (
    <MenuView
      shouldOpenOnLongPress
      onPressAction={({ nativeEvent }) => {
        Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
        const acoes: Record<string, (() => void) | undefined> = { renomear: onRenomear, git: onGit, loop: onLoop, excluir: onExcluir };
        acoes[nativeEvent.event]?.();
      }}
      actions={[
        { id: 'renomear', title: m.sessao_renomear() },
        ...(temCwd ? [{ id: 'git', title: 'Git' }] : []),
        { id: 'loop', title: m.loop_titulo() },
        { id: 'excluir', title: m.sessao_excluir_curto(), attributes: { destructive: true } },
      ]}
    >
      {children}
    </MenuView>
  );
}
