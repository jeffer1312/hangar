import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { State } from '@hangar/core';
import * as m from '../paraglide/messages';
import { StatePill } from '../features/sessions/StatePill';
import { ContextRing } from './ContextRing';
import { Icon } from '../ui/Icon';

// Cabeçalho do chat, na ordem da PWA no celular: voltar · título+chevron · anel · pílula de
// estado · terminal · ⋯. O título é o que cede espaço primeiro, então o chip do plano fica na
// linha de baixo — na mesma linha ele espremia o nome da sessão até as reticências.
export function ChatHeader({
  name,
  state,
  onBack,
  onMore,
  onTitlePress,
  onTerminal,
  contextPct,
  chipLoop,
  chipPlan,
}: {
  name: string;
  state: State | null;
  onBack: () => void;
  onMore: () => void;
  onTitlePress: () => void;
  onTerminal: () => void;
  contextPct?: number | null;
  chipLoop?: React.ReactNode;
  chipPlan?: React.ReactNode;
}) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.wrap}>
      <View style={styles.bar}>
        <Pressable
          onPress={onBack}
          hitSlop={8}
          style={styles.back}
          accessibilityRole="button"
          accessibilityLabel={m.chat_voltar_sessoes()}
        >
          <Icon name="ChevronLeft" size={24} color={theme.tokens.accent.base} />
        </Pressable>
        <Pressable
          onPress={onTitlePress}
          style={styles.titulo}
          accessibilityRole="button"
          accessibilityLabel={m.sessao_trocar_de()}
        >
          <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>
            {name}
          </Text>
          <Icon name="ChevronDown" size={14} color={theme.tokens.text.muted} />
        </Pressable>
        <ContextRing pct={contextPct} />
        {state ? <StatePill state={state} /> : null}
        <Pressable
          onPress={onTerminal}
          hitSlop={8}
          style={styles.more}
          accessibilityRole="button"
          accessibilityLabel={m.term_titulo()}
        >
          <Icon name="Terminal" size={20} color={theme.tokens.text.primary} />
        </Pressable>
        <Pressable
          onPress={onMore}
          hitSlop={8}
          style={styles.more}
          accessibilityRole="button"
          accessibilityLabel={m.navbar_mais_acoes()}
        >
          <Icon name="Ellipsis" size={20} color={theme.tokens.text.primary} />
        </Pressable>
      </View>
      {chipPlan || chipLoop ? (
        <View style={styles.chips}>
          {chipPlan}
          {chipLoop}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    borderBottomWidth: 1,
    borderBottomColor: theme.tokens.border.subtle,
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
    minHeight: 44,
  },
  chips: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[2],
    paddingBottom: theme.base.space[1],
  },
  titulo: {
    flex: 1,
    flexShrink: 1,
    minWidth: 0,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    minHeight: 44,
  },
  back: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  name: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
    flexShrink: 1,
  },
  more: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
}));
