import { memo } from 'react';
import { Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { ManualCategoryId, Server, UpdateNewsKind } from '@hangar/core';
import { useSettingsColors, withAlpha } from '../config/colors';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { Notice } from '../machines/parts';
import { Icon, isIconName, type IconName } from '../../ui/Icon';
import { useSessions } from '../../stores/sessions';
import type { DocState } from './docs';
import * as m from '../../paraglide/messages';

export const kindLabel = (kind: UpdateNewsKind) =>
  kind === 'new' ? m.updates_history_kind_new() : kind === 'improvement' ? m.updates_history_kind_improvement() : m.updates_history_kind_fix();

/** Selo do tipo: verde para novo, destaque para melhoria, âmbar para correção, como o rascunho. */
export const KindPill = memo(function KindPill({ kind }: { kind: UpdateNewsKind }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const color = kind === 'new' ? theme.tokens.status.success : kind === 'improvement' ? c.accentText : theme.tokens.status.warning;
  return <Text style={[styles.pill, { color, backgroundColor: withAlpha(kind === 'improvement' ? c.accent : color, 0.16) }]}>{kindLabel(kind)}</Text>;
});

/** O artigo traz o nome Lucide em kebab-case; nome que o pacote não tem cai no livro, nunca no ícone de alerta. */
export function articleIcon(name: string | null): IconName {
  const pascal = (name ?? '').split('-').map((p) => p.charAt(0).toUpperCase() + p.slice(1)).join('');
  return isIconName(pascal) ? pascal : 'BookOpen';
}

export const categoryLabel = (id: ManualCategoryId) => ({
  comecar: m.manual_category_comecar,
  sessoes: m.manual_category_sessoes,
  ferramentas: m.manual_category_ferramentas,
  problemas: m.manual_category_problemas,
}[id]?.() ?? id);

export function UnseenDot() {
  const c = useSettingsColors();
  return <View style={[styles.dot, { backgroundColor: c.accentText }]} />;
}

/** "● servidor": de qual máquina vem o que a página mostra. */
export function ServerTag({ server }: { server: Server | null }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const bucket = useSessions((s) => s.byServer.find((b) => b.server.id === server?.id));
  if (!server) return null;
  const color = bucket?.error ? theme.tokens.status.error : bucket?.loaded ? theme.tokens.status.success : theme.tokens.text.muted;
  return <View style={styles.server}>
    <View style={[styles.dot, { backgroundColor: color }]} />
    <Text style={[styles.serverText, { color: c.muted }]} numberOfLines={1}>{server.label}</Text>
  </View>;
}

export function SearchField({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  const c = useSettingsColors();
  return <View style={[styles.search, { borderColor: c.border, backgroundColor: c.inset }]}>
    <Icon name="Search" size={14} color={c.faint} />
    <TextInput value={value} onChangeText={onChange} placeholder={placeholder} placeholderTextColor={c.faint}
      accessibilityLabel={placeholder} autoCorrect={false} autoCapitalize="none" clearButtonMode="while-editing"
      returnKeyType="search" style={[styles.searchInput, { color: c.text }]} />
  </View>;
}

/** Os três estados que não são a lista: carregando, servidor antigo e erro com o texto e o tentar de novo. */
export function DocStatus({ state, onRetry }: { state: Exclude<DocState<unknown>, { status: 'ok' }>; onRetry: () => void }) {
  if (state.status === 'loading') return <InfoNotice text={m.comum_carregando()} />;
  if (state.status === 'outdated') return <InfoNotice text={m.updates_history_server_outdated()} />;
  return <View style={styles.error}>
    <Notice text={state.message} error />
    <View style={styles.retry}><Pill label={m.busca_tentar_de_novo()} icon="RefreshCw" onPress={onRetry} /></View>
  </View>;
}

export function EmptyText({ text }: { text: string }) {
  const c = useSettingsColors();
  return <Text style={[styles.empty, { color: c.muted }]}>{text}</Text>;
}

const styles = StyleSheet.create((theme) => ({
  pill: { ...theme.type.badge, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.full, overflow: 'hidden' },
  dot: { width: 8, height: 8, borderRadius: theme.base.radius.full },
  server: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[1] },
  serverText: { flexShrink: 1, fontSize: theme.base.text.xs },
  // 44 pt: alvo de toque mínimo do iOS.
  search: { minHeight: theme.base.space[10] + theme.base.space[1], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.sm },
  searchInput: { flex: 1, alignSelf: 'stretch', fontSize: theme.base.text.sm, padding: 0 },
  error: { gap: theme.base.space[2] },
  retry: { flexDirection: 'row' },
  empty: { paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[2], fontSize: theme.base.text.xs },
}));
