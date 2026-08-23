import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getPlan, getPlans, setPlanPin } from '@hangar/core';
import type { PlanDetail, SessionInfo, PlanListItem } from '@hangar/core';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import { PlanBar } from './PlanBar';
import { PillMenu } from '../pills/PillMenu';
import { currentIndex, taskMark } from './planPanel';
import * as m from '../../paraglide/messages';

interface Props {
  session: SessionInfo | null | undefined;
  name: string;
}

export function PlanPanel({ session, name }: Props) {
  const { theme } = useUnistyles();
  const [detail, setDetail] = useState<PlanDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [showMd, setShowMd] = useState(false);
  const [showTasks, setShowTasks] = useState(true);
  const [plans, setPlans] = useState<PlanListItem[]>([]);
  const [pinned, setPinned] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerErr, setPickerErr] = useState('');
  const genRef = useRef(0);
  const mdStyle = mkMarkdownStyle(theme);

  const fetchPlan = useCallback(async () => {
    const g = ++genRef.current;
    try {
      const d = await getPlan(name);
      if (g !== genRef.current) return;
      setDetail(d);
      setError(false);
      if (d) setLoading(false);
      else setLoading(false);
    } catch {
      if (g !== genRef.current) return;
      setError(true);
      setLoading(false);
    }
  }, [name]);

  useEffect(() => {
    setLoading(true);
    setError(false);
    void fetchPlan();
    const id = setInterval(() => void fetchPlan(), 5000);
    return () => {
      genRef.current += 1;
      clearInterval(id);
    };
  }, [fetchPlan]);

  const loadPlans = useCallback(async () => {
    setPickerErr('');
    try {
      const r = await getPlans(name);
      setPlans(r.plans);
      setPinned(r.pinned);
    } catch (e) {
      setPickerErr(e instanceof Error ? e.message.replace(/^\d+:\s*/, '') : 'falhou');
    }
  }, [name]);

  const handleOpenPicker = useCallback(() => {
    setPickerOpen(true);
    void loadPlans();
  }, [loadPlans]);

  const handleSelect = useCallback(
    async (stem: string) => {
      const alvo = stem || null;
      setPickerErr('');
      try {
        const r = await setPlanPin(name, alvo === '!none' ? '!none' : alvo);
        setPinned(r.pinned);
        setPickerOpen(false);
        void fetchPlan();
      } catch (e) {
        setPickerErr(e instanceof Error ? e.message.replace(/^\d+:\s*/, '') : 'falhou');
      }
    },
    [name, fetchPlan],
  );

  const cur = currentIndex(detail);

  if (!detail && !loading && !error) return null;

  return (
    <View style={styles.wrap}>
      <View style={styles.head}>
        <Pressable onPress={handleOpenPicker} style={[styles.pickBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button" accessibilityLabel={m.plano_trocar()}>
          <Text style={[styles.pickText, { color: theme.tokens.text.secondary }]} numberOfLines={1}>
            {pinned ? plans.find((p) => p.stem === pinned)?.name ?? pinned : session?.plan_name ? `${m.plano_automatico()} · ${session.plan_name}` : m.plano_automatico()}
          </Text>
          <Text style={[styles.caret, { color: theme.tokens.text.muted }]}>▾</Text>
        </Pressable>
        <Pressable onPress={() => setShowMd((v) => !v)} style={styles.chevBtn} accessibilityLabel={showMd ? m.plano_esconder_inteiro() : m.plano_ver_inteiro()}>
          <Text style={[styles.chev, showMd && styles.chevOpen, { color: theme.tokens.text.muted }]}>›</Text>
        </Pressable>
      </View>
      {pickerErr ? <Text style={[styles.err, { color: theme.tokens.status.error }]}>{pickerErr}</Text> : null}

      <Pressable onPress={() => setShowTasks((v) => !v)} style={styles.barBtn} accessibilityRole="button">
        <View style={styles.barWrap}>{detail ? <PlanBar detail={detail} session={session ?? null} /> : <Text style={[styles.barRot, { color: theme.tokens.text.muted }]}>{m.plano_tasks()}</Text>}</View>
        <Text style={[styles.chev, showTasks && styles.chevOpen, { color: theme.tokens.text.muted }]}>›</Text>
      </Pressable>

      {!showTasks ? null : loading && !detail ? (
        <View style={styles.center}>
          <ActivityIndicator color={theme.tokens.text.muted} />
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.plano_carregando()}</Text>
        </View>
      ) : detail ? (
        <View style={styles.tasks}>
          {detail.tasks.map((t, i) => (
            <View key={i} style={styles.taskWrap}>
              <View style={[styles.taskRow, i === cur && styles.taskCur]}>
                <Text style={[styles.mark, t.total > 0 && t.done >= t.total ? { color: theme.tokens.status.success } : i === cur ? { color: theme.tokens.accent.base } : { color: theme.tokens.text.muted }]}>{taskMark(t.done, t.total, i === cur)}</Text>
                <Text style={[styles.taskTitle, { color: i === cur ? theme.tokens.text.primary : theme.tokens.text.muted }]} numberOfLines={1}>
                  {t.title}
                </Text>
                <Text style={[styles.cnt, { color: theme.tokens.text.muted }]}>
                  {t.done}/{t.total}
                </Text>
              </View>
              {i === cur ? (
                <View style={styles.steps}>
                  {t.steps.map((s, si) => (
                    <View key={si} style={[styles.stepRow, s.done && styles.stepDone]}>
                      <Text style={[styles.mark, s.done ? { color: theme.tokens.status.success } : { color: theme.tokens.text.muted }]}>{s.done ? '✓' : '○'}</Text>
                      <Text style={[styles.stepTitle, { color: s.done ? theme.tokens.text.muted : theme.tokens.text.secondary }]} numberOfLines={1}>
                        {s.title}
                      </Text>
                      {s.manual ? <Text style={styles.manual}>🙋</Text> : null}
                    </View>
                  ))}
                </View>
              ) : null}
            </View>
          ))}
        </View>
      ) : error ? (
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.plano_falha_leitura()}</Text>
      ) : null}

      {showMd && detail ? (
        <ScrollView style={[styles.mdBox, { borderColor: theme.tokens.border.subtle, backgroundColor: theme.tokens.bg.surface }]} contentContainerStyle={styles.mdContent}>
          <EnrichedMarkdownText markdown={detail.markdown} markdownStyle={mdStyle} flavor="github" />
        </ScrollView>
      ) : null}

      <PillMenu
        open={pickerOpen}
        onClose={() => setPickerOpen(false)}
        title={m.plano_trocar()}
        loading={false}
        error={pickerErr || null}
        onRetry={loadPlans}
        items={[
          { label: `${m.plano_automatico()}${session?.plan_name ? ` · ${session.plan_name}` : ''}`, hint: '', selected: !pinned },
          { label: m.plano_nenhum(), hint: '', selected: pinned === '!none' },
          ...plans.map((p) => ({ label: p.name, hint: `${p.done}/${p.total}${p.complete ? ' ✓' : ''}`, selected: p.stem === pinned })),
        ]}
        onSelect={(it) => {
          if (it.label === m.plano_automatico() || it.label.startsWith(m.plano_automatico())) void handleSelect('');
          else if (it.label === m.plano_nenhum()) void handleSelect('!none');
          else {
            const hit = plans.find((p) => p.name === it.label);
            void handleSelect(hit?.stem ?? '');
          }
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: theme.base.space[1], paddingVertical: theme.base.space[1] },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minWidth: 0 },
  pickBtn: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], paddingHorizontal: theme.base.space[1], paddingVertical: 2, borderWidth: 1, borderColor: 'transparent', borderRadius: theme.base.radius.sm, minWidth: 0 },
  pickText: { flex: 1, fontSize: theme.base.text.xs, fontWeight: '600', minWidth: 0 },
  caret: { fontSize: 10 },
  chevBtn: { padding: theme.base.space[1] },
  chev: { fontSize: theme.base.text.base, lineHeight: theme.base.text.base },
  chevOpen: { transform: [{ rotate: '90deg' }] },
  err: { fontSize: theme.base.text.xs },
  barBtn: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], paddingVertical: 2 },
  barWrap: { flex: 1, minWidth: 0 },
  barRot: { fontSize: theme.base.text.xs },
  center: { alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[3] },
  muted: { fontSize: theme.base.text.xs, textAlign: 'center' },
  tasks: { gap: 0 },
  taskWrap: { gap: 0 },
  taskRow: { flexDirection: 'row', alignItems: 'baseline', gap: theme.base.space[1], paddingVertical: 3 },
  taskCur: {},
  taskTitle: { flex: 1, fontSize: theme.base.text.xs, minWidth: 0 },
  mark: { width: 16, textAlign: 'center', fontSize: theme.base.text.xs },
  cnt: { fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'], flexShrink: 0 },
  steps: { paddingLeft: theme.base.space[4], gap: 0 },
  stepRow: { flexDirection: 'row', alignItems: 'baseline', gap: theme.base.space[1], paddingVertical: 2 },
  stepDone: { opacity: 0.7 },
  stepTitle: { flex: 1, fontSize: 11, minWidth: 0 },
  manual: { fontSize: 11 },
  mdBox: { maxHeight: 260, borderWidth: 1, borderRadius: theme.base.radius.md, marginTop: theme.base.space[2] },
  mdContent: { padding: theme.base.space[2] },
}));
