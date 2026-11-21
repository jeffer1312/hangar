import type { ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AttachmentOrigin, OriginFilter } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { Icon } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import { originLabel } from './origin';

// Abas, chips e moldura da miniatura recebem só dados: a animação da novidade desenha as mesmas. Sem
// módulo nativo aqui (cache de arquivos, player de áudio, toast): a animação não carrega a tela inteira.

export type MediaTab = 'media' | 'files' | 'audio';

/** Mídia, Arquivos e Áudio, cada uma com a contagem. */
export function MediaTabs({ counts, selected, onSelect }: {
  counts: Record<MediaTab, number>; selected: MediaTab; onSelect?: (tab: MediaTab) => void;
}) {
  const { theme } = useUnistyles();
  const tabs: { id: MediaTab; label: string }[] = [
    { id: 'media', label: m.media_tab_media({ n: counts.media }) },
    { id: 'files', label: m.media_tab_files({ n: counts.files }) },
    { id: 'audio', label: m.media_tab_audio({ n: counts.audio }) },
  ];
  return (
    <View style={[styles.seg, { backgroundColor: superficie(theme, 0.8) }]} accessibilityRole="tablist">
      {tabs.map((t) => (
        <Pressable key={t.id} onPress={onSelect ? () => onSelect(t.id) : undefined}
          style={[styles.segItem, selected === t.id && { backgroundColor: theme.tokens.bg.elevated }]}
          accessibilityRole="tab" accessibilityState={{ selected: selected === t.id }}>
          <Text style={[styles.segTxt, { color: selected === t.id ? theme.tokens.text.primary : theme.tokens.text.secondary }]} numberOfLines={1}>{t.label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

/** Todos, Do agente e Seus. */
export function OriginChips({ selected, onSelect }: { selected: OriginFilter; onSelect?: (filter: OriginFilter) => void }) {
  const { theme } = useUnistyles();
  const chips: { id: OriginFilter; label: string }[] = [
    { id: 'all', label: m.media_filter_all() },
    { id: 'agent', label: m.media_filter_agent() },
    { id: 'user', label: m.media_filter_user() },
  ];
  return (
    <View style={styles.chips}>
      {chips.map((c) => (
        <Pressable key={c.id} onPress={onSelect ? () => onSelect(c.id) : undefined} hitSlop={6}
          style={[styles.chip, { borderColor: selected === c.id ? theme.tokens.accent.base : theme.tokens.border.subtle },
            selected === c.id && { backgroundColor: theme.tokens.accent.dim }]}
          accessibilityRole="button" accessibilityState={{ selected: selected === c.id }}>
          <Text style={[styles.chipTxt, { color: selected === c.id ? theme.tokens.text.primary : theme.tokens.text.secondary }]}>{c.label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

/** A miniatura da grade: `children` é a imagem; vídeo e imagem quebrada mostram o ícone no lugar. */
export function MediaCellFrame({ size, origin, placeholder, onPress, label, children }: {
  size: number; origin: AttachmentOrigin; placeholder?: 'Play' | 'ImageOff'; onPress?: () => void; label?: string; children?: ReactNode;
}) {
  const { theme } = useUnistyles();
  return (
    <Pressable onPress={onPress} style={[styles.cell, { width: size, height: size }]}
      accessibilityRole={onPress ? 'imagebutton' : undefined} accessibilityLabel={label}>
      {placeholder ? (
        <View style={[styles.fill, styles.cellPlaceholder]}>
          <Icon name={placeholder} size={22} color={theme.ink.onAccent} />
        </View>
      ) : children}
      <View style={styles.badge}>
        <Text style={styles.badgeTxt} numberOfLines={1}>{originLabel(origin)}</Text>
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  seg: {
    flexDirection: 'row',
    padding: theme.base.space[0.5],
    borderRadius: theme.base.radius.sm,
    gap: theme.base.space[0.5],
  },
  segItem: {
    flex: 1,
    minHeight: 44,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: theme.base.radius.sm,
  },
  segTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.medium,
    fontVariant: ['tabular-nums'],
  },
  chips: {
    flexDirection: 'row',
    gap: theme.base.space[2],
  },
  chip: {
    minHeight: 32,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  chipTxt: {
    fontSize: theme.base.text.xs,
  },
  cell: {
    overflow: 'hidden',
    backgroundColor: superficie(theme, 0.8),
  },
  fill: {
    width: '100%',
    height: '100%',
  },
  cellPlaceholder: {
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#111',
  },
  badge: {
    position: 'absolute',
    left: theme.base.space[1],
    top: theme.base.space[1],
    paddingHorizontal: theme.base.space[1],
    borderRadius: theme.base.radius.xs,
    backgroundColor: theme.ink.scrim,
  },
  badgeTxt: {
    color: theme.ink.onAccent,
    fontSize: theme.base.text.xxxs,
  },
}));
