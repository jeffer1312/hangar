import { memo, useCallback, type ReactNode } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { reviewNeedsAttention, type ReviewSummary } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { Chip, type Tone } from '../../ui/Chip';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

export function pipelineText(state: string | null): string {
  const labels: Record<string, () => string> = { success: m.native_review_pipeline_success, failed: m.native_review_pipeline_failed,
    running: m.native_review_pipeline_running, pending: m.native_review_pipeline_pending, canceled: m.native_review_pipeline_canceled, skipped: m.native_review_pipeline_skipped };
  return state ? labels[state]?.() ?? state : m.native_review_pipeline_none();
}
export function pipelineTone(state: string | null): Tone {
  return state === 'success' ? 'success' : state === 'failed' ? 'error' : state === 'running' || state === 'pending' ? 'accent' : 'neutral';
}
export const reviewNumber = (review: Pick<ReviewSummary, 'provider' | 'number'>) => `${review.provider === 'gitlab' ? '!' : '#'}${review.number}`;

export function ReviewButton({ label, icon, onPress, disabled = false, primary = false, iconOnly = false, selected }: {
  label: string; icon?: IconName; onPress: () => void; disabled?: boolean; primary?: boolean; iconOnly?: boolean; selected?: boolean;
}) {
  const { theme } = useUnistyles();
  const ink = primary ? theme.tokens.bg.base : theme.tokens.text.primary;
  return <Pressable onPress={onPress} disabled={disabled} accessibilityRole="button" accessibilityLabel={label}
    accessibilityState={{ disabled, ...(selected !== undefined ? { selected } : {}) }} style={({ pressed }) => [styles.button, primary && { backgroundColor: theme.tokens.text.primary }, pressed && styles.pressed, disabled && styles.disabled]}>
    {icon ? <Icon name={icon} size={18} color={ink} /> : null}
    {!iconOnly ? <Text style={[styles.buttonText, { color: ink }]}>{label}</Text> : null}
  </Pressable>;
}

export function ReviewStatus({ pipeline }: { pipeline: string | null }) {
  return <Chip tone={pipelineTone(pipeline)}>{pipelineText(pipeline)}</Chip>;
}

/** A tela e a animação da novidade compartilham esta linha. */
export function ReviewRowView({ review, machine, onPress }: { review: ReviewSummary; machine: string; onPress?: () => void }) {
  const { theme } = useUnistyles();
  return <Pressable onPress={onPress} disabled={!onPress} accessibilityRole={onPress ? 'button' : undefined}
    accessibilityLabel={`${review.title}, ${reviewNumber(review)}, ${machine}, ${pipelineText(review.pipeline)}${reviewNeedsAttention(review) ? `, ${m.native_review_group_attention()}` : ''}`} style={({ pressed }) => [styles.reviewRow, pressed && styles.pressed]}>
    <View style={styles.row}>
      <Icon name="GitPullRequest" size={18} color={theme.tokens.accent.base} />
      <Text style={styles.meta} numberOfLines={1}>{review.repo} {reviewNumber(review)}</Text>
      {reviewNeedsAttention(review) ? <Icon name="CircleDot" size={13} color={theme.tokens.accent.base} /> : null}
    </View>
    <Text style={styles.title} numberOfLines={3}>{review.title}</Text>
    <View style={styles.wrap}>
      <ReviewStatus pipeline={review.pipeline} />
      {review.draft ? <Chip>{m.native_review_state_draft()}</Chip> : null}
      {(review.open_threads ?? 0) > 0 ? <Chip>{m.native_review_threads_open({ n: String(review.open_threads) })}</Chip> : null}
    </View>
    <Text style={styles.meta} numberOfLines={1}>{machine} · {m.native_review_by({ autor: review.author })}</Text>
  </Pressable>;
}

export const ReviewRow = memo(function ReviewRow({ review, serverId, machine }: { review: ReviewSummary; serverId: string; machine: string }) {
  const router = useRouter();
  const open = useCallback(() => router.push({ pathname: '/reviews/detail', params: {
    server: serverId, host: review.host, repo: review.repo, number: String(review.number),
  } } as never), [router, serverId, review.host, review.repo, review.number]);
  return <ReviewRowView review={review} machine={machine} onPress={open} />;
});

export function ReviewSection({ title, children }: { title: string; children: ReactNode }) {
  return <View style={styles.section}><Text accessibilityRole="header" style={styles.heading}>{title}</Text>{children}</View>;
}
export function ReviewNotice({ text, retry, loading = false }: { text: string; retry?: () => void; loading?: boolean }) {
  return <View style={styles.notice}>{loading ? <ActivityIndicator /> : null}<Text style={styles.muted}>{text}</Text>
    {retry ? <ReviewButton label={m.native_retry()} icon="RefreshCw" onPress={retry} /> : null}</View>;
}

export const styles = StyleSheet.create((theme) => ({
  page: { flex: 1 }, content: { padding: theme.base.space[4], gap: theme.base.space[4], paddingBottom: theme.base.space[10] },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  wrap: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[2] },
  button: { minWidth: 48, minHeight: 48, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderRadius: theme.base.radius.md,
    flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: theme.base.space[2] },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium, flexShrink: 1 },
  pressed: { opacity: 0.65 }, disabled: { opacity: 0.4 },
  reviewRow: { paddingVertical: theme.base.space[4], gap: theme.base.space[2], borderBottomWidth: StyleSheet.hairlineWidth, borderBottomColor: theme.tokens.border.default },
  title: { ...theme.type.heading, color: theme.tokens.text.primary }, heading: { ...theme.type.label, color: theme.tokens.text.primary },
  body: { ...theme.type.body, color: theme.tokens.text.primary }, meta: { ...theme.type.meta, color: theme.tokens.text.secondary, flexShrink: 1 },
  muted: { ...theme.type.meta, color: theme.tokens.text.muted },
  section: { gap: theme.base.space[3] }, notice: { padding: theme.base.space[6], alignItems: 'center', gap: theme.base.space[3] },
  search: { minHeight: 48, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, backgroundColor: superficie(theme, 0.6),
    borderWidth: 1, borderColor: theme.tokens.border.default, color: theme.tokens.text.primary, fontSize: theme.base.text.sm },
  group: { borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6) },
  hostRow: { padding: theme.base.space[4], gap: theme.base.space[1], borderBottomWidth: StyleSheet.hairlineWidth, borderBottomColor: theme.tokens.border.default },
  error: { ...theme.type.meta, color: theme.tokens.status.error },
}));
