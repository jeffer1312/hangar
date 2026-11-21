import type { ReactNode } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';

// Partes do visor que recebem só dados: a animação da novidade desenha as mesmas. Sem módulo nativo
// aqui (cache de arquivos, player, toast): quem importa só a aparência não carrega o resto do visor.

export const THUMB_W = 44;
const THUMB_H = 58;

/** `slot`: o download em andamento ocupa o lugar do botão (o anel que cancela), com o mesmo rótulo embaixo. */
export interface LightboxAction { icon: IconName; label: string; a11y: string; onPress: () => void; busy?: boolean; slot?: ReactNode }

/** Nome e, embaixo, "tamanho · data · origem". */
export function LightboxCaption({ filename, details }: { filename: string; details?: string }) {
  return (
    <View style={styles.caption}>
      <Text style={styles.name} numberOfLines={1}>{filename}</Text>
      {details ? <Text style={styles.details} numberOfLines={1}>{details}</Text> : null}
    </View>
  );
}

export function LightboxActions({ actions }: { actions: LightboxAction[] }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.actions}>
      {actions.map((a) => a.slot ? (
        <View key={a.icon} style={styles.action}>
          {a.slot}
          <Text style={styles.actionLabel} numberOfLines={1}>{a.label}</Text>
        </View>
      ) : (
        <Pressable key={a.icon} onPress={a.onPress} disabled={a.busy} style={styles.action}
          accessibilityRole="button" accessibilityLabel={a.a11y} accessibilityState={{ busy: !!a.busy }}>
          {a.busy ? <ActivityIndicator size="small" color={theme.ink.onAccent} /> : <Icon name={a.icon} size={22} color={theme.ink.onAccent} />}
          <Text style={styles.actionLabel} numberOfLines={1}>{a.label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

/** Miniatura da faixa: `children` é a imagem; o vídeo mostra o ▶ no lugar. */
export function StripThumbFrame({ active, kind, onPress, label, children }: {
  active: boolean; kind: 'image' | 'video'; onPress?: () => void; label?: string; children?: ReactNode;
}) {
  const { theme } = useUnistyles();
  return (
    <Pressable
      onPress={onPress}
      style={[styles.thumb, { borderColor: active ? theme.tokens.accent.base : 'transparent', opacity: active ? 1 : 0.55 }]}
      accessibilityRole={onPress ? 'button' : undefined}
      accessibilityState={onPress ? { selected: active } : undefined}
      accessibilityLabel={label}
    >
      {kind === 'image' ? children : (
        <View style={[styles.thumbImg, styles.thumbVideo]}>
          <Icon name="Play" size={14} color={theme.ink.onAccent} />
        </View>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  caption: {
    paddingHorizontal: theme.base.space[4],
    paddingTop: theme.base.space[2],
    gap: theme.base.space[0.5],
  },
  name: {
    color: theme.ink.onAccent,
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.medium,
  },
  details: {
    color: 'rgba(255,255,255,0.62)',
    fontSize: theme.base.text.xxs,
    fontVariant: ['tabular-nums'],
  },
  actions: {
    flexDirection: 'row',
    justifyContent: 'space-around',
    paddingHorizontal: theme.base.space[2],
  },
  action: {
    minWidth: 72,
    minHeight: 52,
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[1],
  },
  actionLabel: {
    color: theme.ink.onAccent,
    fontSize: theme.base.text.xxxs,
  },
  thumb: {
    width: THUMB_W,
    height: THUMB_H,
    borderRadius: theme.base.radius.xs,
    borderWidth: 2,
    overflow: 'hidden',
  },
  thumbImg: {
    width: '100%',
    height: '100%',
  },
  thumbVideo: {
    backgroundColor: '#222',
    alignItems: 'center',
    justifyContent: 'center',
  },
}));
