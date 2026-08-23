import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { kindOf, isPermission } from '@hangar/core';
import * as m from '../paraglide/messages';

interface Props {
  question: string;
  options: string[];
  onSelect: (n: number) => void;
  onCancel: () => void;
}

export function OptionButtons({ question, options, onSelect, onCancel }: Props) {
  const { theme } = useUnistyles();
  const permission = isPermission(options);
  const kinds = options.map((o) => kindOf(o));

  return (
    <View style={styles.wrap}>
      {permission ? (
        <View style={[styles.permChip, { backgroundColor: theme.tokens.accent.dim }]}>
          <Text style={[styles.permTxt, { color: theme.tokens.accent.base }]}>{m.permissao_pedido()}</Text>
        </View>
      ) : null}
      <Text style={[styles.question, { color: theme.tokens.text.primary }]}>
        {question.split('`').map((part, i) =>
          i % 2 === 1 ? (
            <Text key={i} style={[styles.qCode, { backgroundColor: theme.tokens.bg.elevated, color: theme.tokens.text.primary }]}>
              {part}
            </Text>
          ) : (
            part
          ),
        )}
      </Text>
      <View style={styles.list}>
        {options.map((opt, i) => {
          const kind = kinds[i];
          const isAllow = permission && kind === 'allow';
          const isAlways = permission && kind === 'always';
          const isDeny = permission && kind === 'deny';
          return (
            <Pressable
              key={i}
              onPress={() => onSelect(i + 1)}
              style={[
                styles.btn,
                { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.default },
                isAllow && { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.accent.base },
                isAlways && { backgroundColor: theme.tokens.accent.dim, borderColor: theme.tokens.accent.base },
                isDeny && { borderColor: theme.tokens.status.error },
              ]}
              accessibilityRole="button"
            >
              <Text
                style={[
                  styles.num,
                  { color: theme.tokens.text.secondary },
                  isAllow && { color: '#fff' },
                  isAlways && { color: theme.tokens.accent.base },
                  isDeny && { color: theme.tokens.status.error },
                ]}
              >
                {i + 1}.
              </Text>
              <Text
                style={[
                  styles.optTxt,
                  { color: theme.tokens.text.primary },
                  isAllow && { color: '#fff' },
                  isAlways && { color: theme.tokens.accent.base },
                  isDeny && { color: theme.tokens.status.error },
                ]}
              >
                {opt}
              </Text>
            </Pressable>
          );
        })}
        <Pressable
          onPress={onCancel}
          style={[styles.btn, styles.btnCancel, { borderColor: theme.tokens.status.error }]}
          accessibilityRole="button"
        >
          <Text style={[styles.num, { color: theme.tokens.status.error }]}>✕</Text>
          <Text style={[styles.optTxt, { color: theme.tokens.status.error }]}>{m.comum_cancelar()}</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    padding: theme.base.space[3],
    gap: theme.base.space[3],
  },
  permChip: {
    alignSelf: 'flex-start',
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 4,
  },
  permTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  question: {
    fontSize: theme.base.text.base,
    fontWeight: '500',
    lineHeight: 22,
  },
  qCode: {
    fontFamily: theme.base.fontMono,
    fontSize: 13,
    paddingHorizontal: 4,
    borderRadius: 4,
  },
  list: {
    gap: theme.base.space[2],
  },
  btn: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.lg,
  },
  btnCancel: {
    borderColor: theme.tokens.status.error,
  },
  num: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.sm,
    minWidth: 20,
  },
  optTxt: {
    fontSize: theme.base.text.base,
    flex: 1,
  },
}));
