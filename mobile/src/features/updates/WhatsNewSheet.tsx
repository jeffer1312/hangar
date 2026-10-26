import { useEffect, useMemo, useState } from 'react';
import { Pressable, ScrollView, Text, View, useWindowDimensions } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import type { UpdateNews } from '@hangar/core';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import { useSettingsColors } from '../config/colors';
import { Icon } from '../../ui/Icon';
import { animationFor } from './animations';
import { clearPendingNews, readPendingNews } from './pending';
import * as m from '../../paraglide/messages';

// Âmbar do aviso, violeta do selo e fundo do cartão: os tons do mock aprovado, que o tema não tem.
export function useNewsTones() {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return c.dark
    ? { amber: '#f0c46a', amberBg: 'rgba(232,170,60,0.08)', amberLine: 'rgba(232,170,60,0.28)',
      violet: '#c9baf5', violetBg: 'rgba(185,166,240,0.18)', onViolet: theme.tokens.text.inverse,
      card: 'rgba(255,255,255,0.035)', cardLine: 'rgba(255,255,255,0.06)' }
    : { amber: theme.tokens.status.warning, amberBg: 'rgba(232,170,60,0.10)', amberLine: 'rgba(200,140,30,0.35)',
      violet: c.accentText, violetBg: c.accentDim, onViolet: theme.tokens.text.inverse, card: 'rgba(0,0,0,0.03)', cardLine: c.border };
}

/** Markdown do cartão e do aviso: corpo pequeno, na cor pedida. */
export function useSmallMarkdown(color: string) {
  const { theme } = useUnistyles();
  return useMemo(() => {
    const base = mkMarkdownStyle(theme);
    return { ...base, paragraph: { ...base.paragraph, ...theme.type.meta, color } };
  }, [theme, color]);
}

export function SheetHeader({ title, subtitle, onClose }: { title: string; subtitle?: string; onClose: () => void }) {
  const c = useSettingsColors();
  return <View style={styles.heading}>
    <View style={styles.flex}>
      <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{title}</Text>
      {subtitle ? <Text selectable style={[styles.version, { color: c.faint }]}>{subtitle}</Text> : null}
    </View>
    <Pressable accessibilityRole="button" accessibilityLabel={m.sessao_fechar()} onPress={onClose} hitSlop={8} style={styles.close}>
      <Icon name="X" size={21} color={c.muted} />
    </Pressable>
  </View>;
}

export function SectionLabel({ text }: { text: string }) {
  const c = useSettingsColors();
  return <Text accessibilityRole="header" style={[styles.label, { color: c.faint }]}>{text}</Text>;
}

function NewsCard({ item, width }: { item: UpdateNews; width: number }) {
  const c = useSettingsColors();
  const t = useNewsTones();
  const md = useSmallMarkdown(c.muted);
  const Animation = animationFor(item.animation);
  const kind = item.kind === 'new' ? m.update_news_kind_new() : item.kind === 'improvement' ? m.update_news_kind_improvement() : null;
  return <View style={[styles.card, { backgroundColor: t.card, borderColor: t.cardLine }]}>
    {Animation ? <Animation width={width} /> : null}
    <View style={styles.cardTitle}>
      <Text style={[styles.cardName, { color: c.text }]}>{item.title}</Text>
      {kind ? <Text style={[styles.pill, { color: t.violet, backgroundColor: t.violetBg }]}>{kind}</Text> : null}
    </View>
    {item.text ? <EnrichedMarkdownText markdown={item.text} markdownStyle={md} flavor="github" /> : null}
    {item.try_it ? <Text selectable style={[styles.tryIt, { color: t.violet }]}>{m.update_notes_try_it({ text: item.try_it })}</Text> : null}
  </View>;
}

/** Novidades em cartões e correções recolhidas: o mesmo bloco na confirmação e depois de atualizar. */
export function NewsSections({ cards, fixes, label = true }: { cards: UpdateNews[]; fixes: UpdateNews[]; label?: boolean }) {
  const c = useSettingsColors();
  const { width } = useWindowDimensions();
  const [open, setOpen] = useState(false);
  // Largura da folha menos o recuo dela e o do cartão.
  const inner = width - 56;
  return <>
    {cards.length ? <View style={styles.list}>
      {label ? <SectionLabel text={m.update_notes_news()} /> : null}
      {cards.map((item) => <NewsCard key={item.id} item={item} width={inner} />)}
    </View> : null}
    {fixes.length ? <View style={styles.list}>
      <Pressable accessibilityRole="button" accessibilityState={{ expanded: open }} onPress={() => setOpen((v) => !v)} hitSlop={8}
        style={({ pressed }) => [styles.fixToggle, pressed && { opacity: 0.6 }]}>
        <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}><Icon name="ChevronRight" size={15} color={c.muted} /></View>
        <Text style={[styles.fixLabel, { color: c.muted }]}>{m.update_notes_fixes({ n: String(fixes.length) })}</Text>
      </Pressable>
      {open ? fixes.map((item) => <View key={item.id} style={styles.fix}>
        <Text style={[styles.fixText, { color: c.faint }]}>•</Text>
        <View style={styles.flex}>
          <Text selectable style={[styles.fixText, { color: c.text }]}>{item.title}</Text>
          {item.text ? <Text selectable style={[styles.fixText, { color: c.muted }]}>{item.text}</Text> : null}
        </View>
      </View>) : null}
    </View> : null}
  </>;
}

export function PrimaryButton({ label, onPress, disabled }: { label: string; onPress: () => void; disabled?: boolean }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="button" accessibilityState={{ disabled: !!disabled }} disabled={disabled} onPress={onPress}
    style={({ pressed }) => [styles.button, { backgroundColor: c.text, opacity: disabled ? 0.4 : pressed ? 0.8 : 1 }]}>
    <Text style={[styles.buttonLabel, { color: theme.tokens.text.inverse }]}>{label}</Text>
  </Pressable>;
}

export function WhatsNewSheet({ serverId, onClose }: { serverId: string; onClose: () => void }) {
  const insets = useSafeAreaInsets();
  const c = useSettingsColors();
  // Lido uma vez e apagado na hora: a tela aparece uma só vez, mesmo se fechar pelo gesto.
  const [news] = useState(() => readPendingNews(serverId) ?? { cards: [], fixes: [] });
  useEffect(() => { clearPendingNews(serverId); }, [serverId]);
  return <ScrollView contentContainerStyle={[styles.sheet, { paddingBottom: insets.bottom + 24 }]}>
    <SheetHeader title={m.update_whats_new_title()} onClose={onClose} />
    {news.cards.length || news.fixes.length
      ? <NewsSections cards={news.cards} fixes={news.fixes} label={false} />
      : <Text style={[styles.body, { color: c.muted }]}>{m.update_whats_new_empty()}</Text>}
    <View style={styles.actions}>
      <PrimaryButton label={m.sessao_fechar()} onPress={onClose} />
    </View>
  </ScrollView>;
}

export const styles = StyleSheet.create((theme) => ({
  sheet: { padding: theme.base.space[4], gap: theme.base.space[4] },
  heading: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3] },
  title: { ...theme.type.heading },
  version: { ...theme.type.mono, marginTop: theme.base.space[0.5], fontVariant: ['tabular-nums'] },
  close: { width: 44, height: 44, marginTop: -theme.base.space[2], alignItems: 'center', justifyContent: 'center' },
  flex: { flex: 1 },
  label: { fontSize: theme.base.text.xxxs, fontWeight: theme.base.weight.semibold, letterSpacing: 0.8, textTransform: 'uppercase' },
  list: { gap: theme.base.space[2] },
  card: { borderWidth: 1, borderRadius: theme.base.radius.md, borderCurve: 'continuous', padding: theme.base.space[3], gap: theme.base.space[1.5], overflow: 'hidden' },
  cardTitle: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[2] },
  cardName: { flexShrink: 1, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  pill: { ...theme.type.badge, fontWeight: theme.base.weight.semibold, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.full, overflow: 'hidden' },
  tryIt: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  fixToggle: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], alignSelf: 'flex-start' },
  fixLabel: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  fix: { flexDirection: 'row', gap: theme.base.space[2], paddingLeft: theme.base.space[1] },
  fixText: { ...theme.type.meta },
  body: { fontSize: theme.base.text.sm, lineHeight: 21 },
  actions: { flexDirection: 'row', gap: theme.base.space[3] },
  button: { minHeight: 48, flex: 1, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.md, borderCurve: 'continuous', paddingHorizontal: theme.base.space[4] },
  buttonLabel: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
}));
