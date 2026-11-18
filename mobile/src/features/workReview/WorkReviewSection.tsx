import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { ActivityIndicator, FlatList, Linking, Pressable, Text, View } from 'react-native';
import { Image } from 'expo-image';
import Animated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { pendingCount, relativeTime, stateTone, stepStatus, waitingHeadlineText } from '@hangar/core';
import type { Server, WaitingHeadlineText, WorkReview, WorkReviewResolvedItem, WorkReviewSteps, WorkReviewState, WorkReviewTone } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { CompareCurtain, type CurtainItem } from './CompareCurtain';
import { DecisionCard } from './DecisionCard';
import { useWorkReview, type WorkReviewController } from './useWorkReview';
import { workReviewAssetSource } from './api';
import { toneColors } from './tones';
import * as m from '../../paraglide/messages';

type Props = {
  serverId: string;
  name: string;
  /** Folha aberta pelo chip: carregando e erro aparecem mesmo antes de a lista dizer que há revisão. */
  standalone?: boolean;
};

/** Seção da revisão do trabalho (folha do celular). Sem vínculo ou sem revisão, não desenha nada. */
export function WorkReviewSection({ serverId, name, standalone = false }: Props) {
  const state = useWorkReview(serverId, name);
  return <WorkReviewView state={state} name={name} standalone={standalone} />;
}

/** A sessão tem revisão: a lista a anunciou ou a leitura a trouxe. É o que decide a aba Revisão do tablet. */
export function hasWorkReview({ summary, review, server, load }: WorkReviewController): boolean {
  return !!summary || (!!review && !!server && load.status === 'ready' && load.data.linked && load.data.supported);
}

type ViewProps = {
  state: WorkReviewController;
  name: string;
  standalone?: boolean;
  /** Aba própria do tablet: a altura toda é da revisão, e o quadro largo cresce. */
  tall?: boolean;
};

/** O corpo da seção, com o estado vindo de quem chamou `useWorkReview` (o painel do tablet também lê o pendente para a aba). */
export function WorkReviewView({ state, name, standalone = false, tall = false }: ViewProps) {
  const { theme } = useUnistyles();
  const { load, review, server, summary } = state;
  // Sem o resumo da lista, a sessão pode nem ter vínculo: a leitura em curso ou falha não ganha espaço.
  const announced = standalone || !!summary;

  if (load.status === 'loading') {
    return announced ? <View style={styles.feedback}><ActivityIndicator color={theme.tokens.text.muted} accessibilityLabel={m.comum_carregando()} /></View> : null;
  }
  if (load.status === 'error') {
    return announced ? (
      <View style={styles.feedback}>
        <Text style={styles.error} accessibilityRole="alert">{load.message}</Text>
        <Pressable onPress={state.reload} style={styles.retry} accessibilityRole="button" accessibilityLabel={m.native_retry()}>
          <Icon name="RotateCw" size={14} color={theme.tokens.accent.base} />
          <Text style={styles.retryText}>{m.native_retry()}</Text>
        </Pressable>
      </View>
    ) : null;
  }
  const data = load.data;
  if (!data.linked || !data.supported || !review || !server) {
    // Sem anúncio a seção some; a folha aberta pelo chip precisa dizer que a revisão acabou.
    return standalone ? <Text style={styles.muted}>{m.work_review_empty()}</Text> : null;
  }
  return <ReviewBody review={review} server={server} name={name} branch={data.via === 'branch'} state={state} tall={tall} />;
}

const stripKey = (item: CurtainItem | null, k: number) => (item ? `${k}:${item.title}` : 'more');

function headlineText(kind: WaitingHeadlineText, connector: string | undefined): string | undefined {
  if (kind === 'ready') return m.work_review_ready();
  if (kind === 'waiting_one') return m.work_review_waiting_one();
  if (kind === 'waiting') return m.work_review_waiting();
  return connector;
}

function ReviewBody({ review, server, name, branch, state, tall }: {
  review: WorkReview; server: Server; name: string; branch: boolean; state: WorkReviewController; tall: boolean;
}) {
  const [curtain, setCurtain] = useState<{ items: CurtainItem[]; index: number } | null>(null);
  const [resolvedOpen, setResolvedOpen] = useState(true);
  const [keptOpen, setKeptOpen] = useState(false);
  const asset = useMemo(() => {
    const cache = new Map<string, { uri: string; headers: Record<string, string> }>();
    return (id: string) => {
      let source = cache.get(id);
      if (!source) { source = workReviewAssetSource(server, name, review.id, id); cache.set(id, source); }
      return source;
    };
  }, [server, name, review.id]);
  const waiting = review.state === 'waiting' && review.decisions.length > 0;
  const pending = pendingCount(review, state.choices);
  const updated = Date.parse(review.updated_at);

  const resolvedItems = useMemo<CurtainItem[]>(() => (review.resolved?.items ?? []).map((item: WorkReviewResolvedItem) => ({
    title: item.title, before: item.before ? asset(item.before) : null, after: item.after ? asset(item.after) : null,
    leftTag: m.work_review_before(), rightTag: m.work_review_after(),
  })), [review.resolved, asset]);
  const openDecision = () => {
    const d = review.decisions[state.index];
    if (!d?.left) return;
    setCurtain({ index: 0, items: [{
      title: d.right ? m.work_review_compare_title({ title: d.title, left: d.left.label, right: d.right.label }) : d.title,
      before: asset(d.left.asset), after: d.right ? asset(d.right.asset) : null,
      leftTag: d.left.label, rightTag: d.right?.label ?? '', aspect: d.left.height / d.left.width,
    }] });
  };

  const resolvedCount = review.resolved ? review.resolved.count ?? review.resolved.items.length : 0;
  const more = review.resolved ? resolvedCount - review.resolved.items.length : 0;
  // Só os quadros perto da tela montam: são até 200 pares de imagens grandes. `null` no fim é o quadro "+N".
  const strip = useMemo<(CurtainItem | null)[]>(() => (more > 0 ? [...resolvedItems, null] : resolvedItems), [resolvedItems, more]);

  return (
    <View style={styles.section}>
      <View style={styles.header}>
        <Text style={styles.title} accessibilityRole="header">{review.title}</Text>
        {review.subject ? <Text style={styles.subject} numberOfLines={1}>{review.subject}</Text> : null}
        {Number.isFinite(updated) ? <Text style={styles.when}>{relativeTime(updated / 1000)}</Text> : null}
      </View>
      {branch ? <Text style={styles.note}>{m.work_review_found_by_branch()}</Text> : null}

      {review.steps ? <StepsTrail steps={review.steps} state={review.state} /> : null}

      <HeadlineBand review={review} pending={pending} />

      {review.state === 'running' && review.progress ? (
        <View style={styles.progress}>
          <View style={styles.row}>
            <Text style={styles.muted}>{review.progress.label}</Text>
            <Text style={styles.mono}>{m.work_review_position({ index: review.progress.done, total: review.progress.total })}</Text>
          </View>
          <View accessibilityRole="progressbar" accessibilityLabel={review.progress.label}
            accessibilityValue={{ min: 0, max: review.progress.total, now: review.progress.done }} style={styles.track}>
            <View style={[styles.fill, { width: `${review.progress.total > 0 ? Math.min(100, (review.progress.done / review.progress.total) * 100) : 0}%` }]} />
          </View>
        </View>
      ) : null}

      {waiting ? (
        <DecisionCard review={review} choices={state.choices} index={state.index} sending={state.sending} sendError={state.sendError} tall={tall}
          asset={asset} onIndex={state.setIndex} onPick={state.pick} onReason={state.setReason} onSend={state.send}
          onCompare={openDecision} onAssetError={state.onAssetError} />
      ) : state.sendError ? <Text style={styles.error} accessibilityRole="alert">{state.sendError}</Text> : null}

      {/* O desenho tira as corrigidas enquanto roda: a lista ainda está mudando. */}
      {review.resolved && review.state !== 'running' ? (
        <Collapsible label={review.resolved.label} count={resolvedCount} hint={m.work_review_drag_hint()} open={resolvedOpen}
          onToggle={() => setResolvedOpen((v) => !v)}>
          <FlatList horizontal data={strip} keyExtractor={stripKey} showsHorizontalScrollIndicator={false}
            contentContainerStyle={styles.strip} initialNumToRender={4} maxToRenderPerBatch={4} windowSize={3}
            renderItem={({ item, index: k }) => item ? (
              <Pressable disabled={!item.before && !item.after} accessibilityRole="imagebutton"
                accessibilityLabel={item.title} onPress={() => setCurtain({ items: resolvedItems, index: k })} style={styles.pair}>
                <View style={styles.pairImage}>
                  {item.before ? <Image source={item.before} recyclingKey={item.before.uri} contentFit="cover" contentPosition="top left"
                    style={styles.pairFull} onError={state.onAssetError} /> : null}
                  {item.after ? <View style={[styles.pairHalf, !item.before && styles.pairWhole]}>
                    <Image source={item.after} recyclingKey={item.after.uri} contentFit="cover" contentPosition="top left"
                      style={[styles.pairShifted, !item.before && styles.pairUnshifted]} onError={state.onAssetError} />
                  </View> : null}
                  {item.before && item.after ? <View style={styles.pairDivider} /> : null}
                </View>
                <Text style={styles.caption} numberOfLines={1}>{item.title}</Text>
              </Pressable>
            ) : <View style={styles.moreTile}><Text style={styles.muted}>{m.work_review_more({ count: more })}</Text></View>} />
        </Collapsible>
      ) : null}

      {review.kept ? (
        <Collapsible label={review.kept.label} count={review.kept.count ?? review.kept.items.length} open={keptOpen}
          onToggle={() => setKeptOpen((v) => !v)}>
          <View style={styles.kept}>
            {review.kept.items.map((item, k) => (
              <View key={`${k}:${item.title}`} style={styles.keptRow}>
                <Text style={styles.keptTitle}>{item.title}</Text>
                <Text style={styles.keptReason}>{item.reason}</Text>
              </View>
            ))}
          </View>
        </Collapsible>
      ) : null}

      {review.links.length ? (
        <View style={styles.links}>
          {review.links.map((link) => link.local ? (
            // O link local abre só na máquina do servidor, que nunca é o aparelho.
            <View key={link.url} style={styles.link} accessible accessibilityLabel={`${link.label}, ${m.work_review_local_link()}`}>
              <Text style={styles.linkDisabled}>{link.label}</Text>
              <Text style={styles.caption}>{m.work_review_local_link()}</Text>
            </View>
          ) : (
            <Pressable key={link.url} style={styles.link} accessibilityRole="link" accessibilityLabel={link.label}
              onPress={() => void Linking.openURL(link.url).catch((e: unknown) => toast.erro(e instanceof Error ? e.message : String(e)))}>
              <Text style={styles.linkText}>{link.label}</Text>
            </Pressable>
          ))}
        </View>
      ) : null}

      {curtain ? <CompareCurtain items={curtain.items} index={curtain.index}
        onIndex={(index) => setCurtain((c) => (c ? { ...c, index } : c))} onClose={() => setCurtain(null)} /> : null}
    </View>
  );
}

/** Faixa de chamada no tom do estado; esperando, o número é o que ainda falta decidir aqui. Sem `headline` e fora da
 *  espera, não aparece. */
export function HeadlineBand({ review, pending }: { review: WorkReview; pending: number }) {
  const { theme } = useUnistyles();
  const tone = stateTone(review.state);
  const colors = toneColors(theme, tone);
  const headline = review.headline;
  const waiting = review.state === 'waiting' && review.decisions.length > 0;
  if (!headline && !waiting) return null;
  const bigValue = waiting ? String(pending) : headline?.value;
  const bigText = waiting ? headlineText(waitingHeadlineText(headline, pending), headline?.text) : headline?.text;
  return (
    <View style={[styles.headline, { backgroundColor: colors.surface }]}>
      {bigValue ? <Text style={[styles.big, { color: tone === 'accent' ? theme.tokens.text.primary : colors.fg }]}>{bigValue}</Text> : null}
      <View style={styles.headlineText}>
        {bigText ? <Text style={styles.headlineTitle}>{bigText}</Text> : null}
        {headline?.detail ? <Text style={styles.muted}>{headline.detail}</Text> : null}
      </View>
    </View>
  );
}

function Collapsible({ label, count, hint, open, onToggle, children }: {
  label: string; count: number; hint?: string; open: boolean; onToggle: () => void; children: ReactNode;
}) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.block}>
      <Pressable onPress={onToggle} accessibilityRole="button" accessibilityState={{ expanded: open }}
        accessibilityLabel={`${label}, ${count}`} style={styles.summary}>
        <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
        <Text style={styles.summaryLabel}>{label}</Text>
        <Text style={styles.mono}>{count}</Text>
        {hint ? <Text style={styles.summaryHint}>{hint}</Text> : null}
      </Pressable>
      {open ? children : null}
    </View>
  );
}

function StepsTrail({ steps, state }: { steps: WorkReviewSteps; state: WorkReviewState }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.trail}>
      {steps.labels.map((label, k) => {
        const step = stepStatus(steps, state, k);
        const lineBefore = k > 0 && step.status !== 'pending';
        const lineAfter = k < steps.labels.length - 1 && stepStatus(steps, state, k + 1).status !== 'pending';
        const fg = step.tone ? toneColors(theme, step.tone).fg : null;
        return (
          <View key={`${k}:${label}`} style={styles.step}>
            <View style={styles.stepLine}>
              <View style={[styles.wire, k === 0 && styles.hidden, lineBefore && styles.wireDone]} />
              <StepDot status={step.status} tone={step.tone} pulse={step.pulse} />
              <View style={[styles.wire, k === steps.labels.length - 1 && styles.hidden, lineAfter && styles.wireDone]} />
            </View>
            <Text numberOfLines={2} style={[styles.stepLabel, step.status === 'current' && {
              color: step.tone === 'accent' ? theme.tokens.text.primary : fg ?? theme.tokens.text.muted, fontWeight: theme.base.weight.semibold,
            }]}>{label}</Text>
          </View>
        );
      })}
    </View>
  );
}

function StepDot({ status, tone, pulse }: { status: 'done' | 'current' | 'pending'; tone: WorkReviewTone | null; pulse: boolean }) {
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const ring = useSharedValue(0);
  const animate = pulse && !reduced;
  useEffect(() => {
    if (!animate) { cancelAnimation(ring); ring.value = 0; return; }
    ring.value = withRepeat(withTiming(1, { duration: 1200 }), -1, false);
    return () => cancelAnimation(ring);
  }, [animate, ring]);
  // O anel cresce e some na thread de UI, como o `pulso` do desenho.
  const ringStyle = useAnimatedStyle(() => ({ opacity: 1 - ring.value, transform: [{ scale: 1 + ring.value * 0.6 }] }));
  const colors = tone ? toneColors(theme, tone) : null;
  return (
    <View style={styles.dotSlot}>
      {status === 'current' && colors ? <Animated.View style={[styles.ring, { backgroundColor: colors.surface }, animate ? ringStyle : null]} /> : null}
      <View style={[styles.stepDot, colors ? { backgroundColor: colors.fg, borderColor: colors.fg } : null]} />
    </View>
  );
}

const PAIR = { width: 150, height: 84 };

const styles = StyleSheet.create((theme) => ({
  section: { gap: theme.base.space[3] },
  feedback: { gap: theme.base.space[2], paddingVertical: theme.base.space[2] },
  error: { ...theme.type.meta, color: theme.tokens.status.error },
  retry: { alignSelf: 'flex-start', minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  retryText: { ...theme.type.control, color: theme.tokens.accent.base },
  header: { flexDirection: 'row', alignItems: 'baseline', flexWrap: 'wrap', gap: theme.base.space[2] },
  title: { ...theme.type.label, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  subject: { flexShrink: 1, ...theme.type.meta, color: theme.tokens.text.muted },
  when: { marginLeft: 'auto', ...theme.type.caption, fontFamily: theme.base.fontMono, color: theme.tokens.text.muted },
  note: { ...theme.type.caption, color: theme.tokens.text.muted },
  trail: { flexDirection: 'row' },
  step: { flex: 1, minWidth: 0, alignItems: 'center', gap: theme.base.space[1] },
  stepLine: { alignSelf: 'stretch', flexDirection: 'row', alignItems: 'center' },
  wire: { flex: 1, height: 2, backgroundColor: theme.tokens.border.default },
  wireDone: { backgroundColor: theme.tokens.status.success },
  hidden: { opacity: 0 },
  dotSlot: { width: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
  ring: { position: 'absolute', width: 20, height: 20, borderRadius: theme.base.radius.full },
  stepDot: { width: 12, height: 12, borderRadius: theme.base.radius.full, borderWidth: 2, borderColor: theme.tokens.border.default },
  stepLabel: { ...theme.type.badge, fontWeight: theme.base.weight.normal, color: theme.tokens.text.muted, textAlign: 'center' },
  headline: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], padding: theme.base.space[3], borderRadius: theme.base.radius.md },
  big: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xl, fontWeight: theme.base.weight.bold },
  headlineText: { flex: 1, gap: theme.base.space[0.5] },
  headlineTitle: { ...theme.type.label, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  muted: { ...theme.type.meta, color: theme.tokens.text.muted },
  mono: { ...theme.type.mono, color: theme.tokens.text.muted },
  row: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: theme.base.space[2] },
  progress: { gap: theme.base.space[1.5] },
  track: { height: 6, overflow: 'hidden', borderRadius: theme.base.radius.full, backgroundColor: theme.tokens.fillSubtle },
  fill: { height: '100%', backgroundColor: theme.tokens.accent.base },
  block: { borderTopWidth: 1, borderTopColor: theme.tokens.border.subtle, paddingTop: theme.base.space[1] },
  summary: { minHeight: 44, flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[2] },
  summaryLabel: { flexShrink: 1, ...theme.type.meta, color: theme.tokens.text.primary },
  summaryHint: { marginLeft: 'auto', ...theme.type.caption, color: theme.tokens.text.muted },
  strip: { gap: theme.base.space[2], paddingVertical: theme.base.space[1] },
  pair: { width: PAIR.width, gap: theme.base.space[1] },
  pairImage: { position: 'relative', width: PAIR.width, height: PAIR.height, overflow: 'hidden', borderWidth: 1,
    borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.sm, backgroundColor: theme.tokens.bg.surface },
  pairFull: { position: 'absolute', left: 0, top: 0, width: PAIR.width, height: PAIR.height },
  pairHalf: { position: 'absolute', top: 0, bottom: 0, left: PAIR.width / 2, right: 0, overflow: 'hidden' },
  pairWhole: { left: 0 },
  pairShifted: { position: 'absolute', top: 0, left: -PAIR.width / 2, width: PAIR.width, height: PAIR.height },
  pairUnshifted: { left: 0 },
  pairDivider: { position: 'absolute', top: 0, bottom: 0, left: PAIR.width / 2 - 1, width: 2, backgroundColor: theme.tokens.accent.base },
  caption: { ...theme.type.caption, color: theme.tokens.text.muted },
  moreTile: { width: PAIR.width, height: PAIR.height, alignItems: 'center', justifyContent: 'center', borderWidth: 1,
    borderStyle: 'dashed', borderColor: theme.tokens.border.strong, borderRadius: theme.base.radius.sm },
  kept: { gap: theme.base.space[1], paddingBottom: theme.base.space[1] },
  keptRow: { flexDirection: 'row', gap: theme.base.space[2], paddingVertical: theme.base.space[1], borderBottomWidth: 1,
    borderBottomColor: theme.tokens.border.subtle },
  keptTitle: { flex: 1, ...theme.type.meta, color: theme.tokens.text.primary },
  keptReason: { flexShrink: 1, maxWidth: '50%', ...theme.type.meta, color: theme.tokens.text.muted, textAlign: 'right' },
  links: { flexDirection: 'row', flexWrap: 'wrap', columnGap: theme.base.space[4] },
  link: { minHeight: 44, justifyContent: 'center' },
  linkText: { ...theme.type.meta, color: theme.tokens.text.secondary, textDecorationLine: 'underline' },
  linkDisabled: { ...theme.type.meta, color: theme.tokens.text.muted },
}));
