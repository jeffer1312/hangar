import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { pendingCount, stateTone, type AggSession } from '@hangar/core';
import { Glass } from '../../ui/Glass';
import { useServers } from '../../stores/servers';
import { FilesPanel } from '../files/FilesPanel';
import { ActivityPanel } from '../activity/ActivityPanel';
import { GitPanel } from '../git/GitPanel';
import { useWorkReview, type WorkReviewController } from '../workReview/useWorkReview';
import { WorkReviewView, hasWorkReview } from '../workReview/WorkReviewSection';
import { badgeColor, toneColors } from '../workReview/tones';
import { ContextTab } from './ContextTab';
import { usePanelTabRequest, type PanelTab } from './panelTab';
import * as m from '../../paraglide/messages';

type Props = { serverId: string; name: string; focused: boolean; onPick?: (session: AggSession) => void };
const tabs: { value: PanelTab; label: () => string }[] = [
  { value: 'context', label: () => m.tablet_tab_context() },
  { value: 'files', label: () => m.tablet_tab_files() },
  { value: 'review', label: () => m.work_review_tab() },
  { value: 'background', label: () => m.tablet_tab_background() },
  { value: 'git', label: () => m.tablet_tab_git() },
];

export function ContextPanel({ serverId, name, focused, onPick }: Props) {
  const sessionKey = `${serverId}::${name}`;
  // O chip da revisão pede a aba antes de a rota trocar; a sessão nova nasce nela, as outras em Contexto.
  const requested = usePanelTabRequest((s) => (s.request?.key === sessionKey ? s.request.tab : null));
  const clearRequest = usePanelTabRequest((s) => s.clear);
  const [selection, setSelection] = useState<{ key: string; tab: PanelTab }>(() => ({ key: sessionKey, tab: requested ?? 'context' }));
  if (selection.key !== sessionKey) setSelection({ key: sessionKey, tab: requested ?? 'context' });
  useEffect(() => {
    if (!requested) return;
    setSelection({ key: sessionKey, tab: requested });
    clearRequest(sessionKey);
  }, [requested, clearRequest, sessionKey]);
  const review = useWorkReview(serverId, name);
  const reviewShown = hasWorkReview(review);
  const selected = selection.key === sessionKey ? selection.tab : requested ?? 'context';
  // A revisão acabou com a aba aberta: volta ao Contexto em vez de deixar o painel vazio.
  if (selection.key === sessionKey && selected === 'review' && !reviewShown) setSelection({ key: sessionKey, tab: 'context' });
  const tab = selected === 'review' && !reviewShown ? 'context' : selected;
  const ready = useServers((s) => s.ready);
  const server = useServers((s) => s.servers.find((item) => item.id === serverId));

  return (
    <Glass variant="panel" style={styles.panel}>
      <View style={styles.tabs} accessibilityRole="tablist">
        {tabs.filter((item) => item.value !== 'review' || reviewShown).map((item) => <Pressable key={item.value}
          accessibilityRole="tab" accessibilityLabel={item.label()}
          accessibilityValue={item.value === 'review' && review.summary?.badge ? { text: review.summary.badge.text } : undefined}
          accessibilityState={{ selected: tab === item.value }} onPress={() => setSelection({ key: sessionKey, tab: item.value })}
          style={[styles.tab, tab === item.value && styles.selectedTab]}>
          <View>
            <Text style={[styles.tabLabel, tab === item.value && styles.selectedLabel]}>{item.label()}</Text>
            {item.value === 'review' ? <ReviewTabBadge state={review} /> : null}
          </View>
        </Pressable>)}
      </View>
      <View key={sessionKey} style={styles.body}>
        {tab === 'context' ? <ContextTab serverId={serverId} name={name} onPick={onPick} />
          : tab === 'review' ? <ScrollView style={styles.body} contentContainerStyle={styles.review} keyboardShouldPersistTaps="handled">
            <WorkReviewView state={review} name={name} tall />
          </ScrollView>
          : !ready ? <View style={styles.feedback}><ActivityIndicator accessibilityLabel={m.comum_carregando()} /></View>
          : !server ? <View style={styles.feedback}><Text style={styles.error} accessibilityRole="alert">{m.chat_servidor_removido()}</Text></View>
          : tab === 'files' ? <FilesPanel serverId={serverId} name={name} />
          : tab === 'background' ? <ActivityPanel serverId={serverId} name={name} focused={focused && tab === 'background'} />
          : <GitPanel server={server} name={name} />}
      </View>
    </Glass>
  );
}

/** Marca depois do rótulo: pendentes enquanto espera, ponto rodando, ✓ concluída. */
function ReviewTabBadge({ state }: { state: WorkReviewController }) {
  const { theme } = useUnistyles();
  const { review, summary, choices } = state;
  const status = review?.state ?? summary?.state;
  if (!status) return null;
  if (status === 'done') return <Text testID="review-tab-done" style={[styles.badge, styles.check, { color: theme.tokens.status.success }]}>✓</Text>;
  if (status === 'waiting') {
    const colors = toneColors(theme, 'warning');
    // O número só com a revisão lida: o resumo da lista não traz as decisões.
    const pending = review?.state === 'waiting' ? pendingCount(review, choices) : 0;
    return pending > 0
      ? <Text testID="review-tab-count" style={[styles.badge, styles.count, { color: colors.fg, backgroundColor: colors.surface }]}>{pending}</Text>
      : <View testID="review-tab-dot" style={[styles.badge, styles.dot, { backgroundColor: colors.fg }]} />;
  }
  const color = status === 'running' ? theme.tokens.accent.base
    : summary?.badge ? badgeColor(theme, summary.badge.tone) : toneColors(theme, stateTone(status)).fg;
  return <View testID="review-tab-dot" style={[styles.badge, styles.dot, { backgroundColor: color }]} />;
}

const styles = StyleSheet.create((theme) => ({
  panel: { width: 300, flexShrink: 0, minHeight: 0 },
  tabs: { flexDirection: 'row', borderBottomWidth: 1, borderBottomColor: theme.tokens.border.subtle },
  tab: { flex: 1, minHeight: 44, alignItems: 'center', justifyContent: 'center',
    padding: theme.base.space[1], borderBottomWidth: 2, borderBottomColor: 'transparent' },
  selectedTab: { borderBottomColor: theme.tokens.accent.base },
  tabLabel: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary, textAlign: 'center' },
  selectedLabel: { color: theme.tokens.accent.base, fontWeight: theme.base.weight.semibold },
  // Fora do fluxo, logo depois do rótulo: com cinco abas em 300 pt, a marca na linha cortaria o "Revisão".
  badge: { position: 'absolute', left: '100%', top: -4, marginLeft: 1 },
  dot: { top: 0, width: 6, height: 6, borderRadius: theme.base.radius.full },
  count: { minWidth: 14, paddingHorizontal: 3, overflow: 'hidden', borderRadius: theme.base.radius.full, textAlign: 'center',
    ...theme.type.badge },
  check: { ...theme.type.badge },
  body: { flex: 1, minHeight: 0 },
  review: { padding: theme.base.space[4] },
  feedback: { flex: 1, padding: theme.base.space[4], alignItems: 'center', justifyContent: 'center' },
  error: { fontSize: theme.base.text.xs, color: theme.tokens.status.error },
}));
