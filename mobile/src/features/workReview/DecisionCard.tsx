import { useEffect, useRef, type ReactNode } from 'react';
import { ActivityIndicator, Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { pendingCount, readyToSend } from '@hangar/core';
import type { WorkReview, WorkReviewChoice, WorkReviewChoices, WorkReviewSide } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { MultilineInput } from '../../ui/MultilineInput';
import { ZoomFrame } from './ZoomFrame';
import { toneColors } from './tones';
import * as m from '../../paraglide/messages';

type Source = { uri: string; headers: Record<string, string> };

type Props = {
  review: WorkReview;
  choices: WorkReviewChoices;
  index: number;
  sending: boolean;
  sendError: string | null;
  /** Aba Revisão do tablet: o quadro largo ganha altura. O par fica igual, cada lado já é estreito no painel. */
  tall?: boolean;
  asset: (asset: string) => Source;
  onIndex: (index: number) => void;
  onPick: (decisionId: string, choice: WorkReviewChoice) => void;
  onReason: (decisionId: string, reason: string) => void;
  onSend: () => void;
  onCompare: () => void;
  onAssetError: () => void;
  /** Desenho de cada lado no lugar da captura (animação da novidade). */
  drawing?: (side: WorkReviewSide) => ReactNode;
};

// O mesmo respiro do desenho entre escolher e mostrar a próxima.
const ADVANCE_MS = 250;
const FRAME_HEIGHT = { pair: 120, wide: 150, wideTall: 200 };

/** Uma decisão por vez: posição, trecho com zoom, escolhas, motivo e envio de todas juntas. */
export function DecisionCard({ review, choices, index, sending, sendError, tall = false, asset, onIndex, onPick, onReason, onSend, onCompare, onAssetError, drawing }: Props) {
  const { theme } = useUnistyles();
  const decision = review.decisions[index];
  const total = review.decisions.length;
  const picked = decision ? choices[decision.id] : undefined;
  const option = decision?.choices.find((c) => c.id === picked?.choice);
  const pending = pendingCount(review, choices);
  const ready = readyToSend(review, choices);
  const input = useRef<TextInput>(null);
  const advance = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(advance.current), []);
  if (!decision) return null;

  const choose = (choice: WorkReviewChoice) => {
    clearTimeout(advance.current);
    onPick(decision.id, choice);
    if (choice.requires_reason) setTimeout(() => input.current?.focus(), 0);
    else if (index < total - 1) advance.current = setTimeout(() => onIndex(index + 1), ADVANCE_MS);
  };
  const frame = (side: WorkReviewSide, height: number) => (
    <View style={styles.side}>
      <View style={styles.caption}>
        <Text style={styles.captionLabel} numberOfLines={1}>{side.label}</Text>
        {side.value ? <Text style={styles.captionValue} numberOfLines={1}>{side.value}</Text> : null}
      </View>
      {/* A chave é a URL: rodada nova traz imagem nova e o quadro quebrado volta a tentar. */}
      <ZoomFrame key={asset(side.asset).uri} side={side} source={asset(side.asset)} height={height} onPress={onCompare} onError={onAssetError}
        drawing={drawing?.(side)} label={decision.right
          ? m.work_review_compare_title({ title: decision.title, left: decision.left?.label ?? '', right: decision.right.label })
          : `${decision.title}: ${side.label}`} />
    </View>
  );
  const sendLabel = pending > 1 ? m.work_review_pending({ count: pending }) : pending === 1 ? m.work_review_pending_one() : m.work_review_send();
  const reasonMissing = !!option?.requires_reason && !picked?.reason.trim();

  return (
    <View style={[styles.card, { borderColor: `${theme.tokens.status.warning}80` }]}>
      <View style={styles.top}>
        {decision.tag ? <Text style={styles.tag} numberOfLines={1}>{decision.tag}</Text> : null}
        <View style={styles.position} accessibilityLabel={m.work_review_position({ index: index + 1, total })} accessible>
          {review.decisions.map((d, k) => (
            <View key={d.id} style={[styles.dot, choices[d.id] ? { backgroundColor: theme.tokens.status.success }
              : k === index ? { backgroundColor: theme.tokens.status.warning } : null]} />
          ))}
          <Text style={styles.count}>{m.work_review_position({ index: index + 1, total })}</Text>
        </View>
      </View>
      <Text style={styles.title}>{decision.title}</Text>
      {decision.reason ? (
        <Text style={styles.reason}>
          {decision.reason_label ? `${decision.reason_label}: ` : ''}
          <Text style={styles.reasonStrong}>{decision.reason}</Text>
        </Text>
      ) : null}

      {decision.left && decision.right ? (
        <View style={styles.sides}>{frame(decision.left, FRAME_HEIGHT.pair)}{frame(decision.right, FRAME_HEIGHT.pair)}</View>
      ) : decision.left ? <View style={styles.sides}>{frame(decision.left, tall ? FRAME_HEIGHT.wideTall : FRAME_HEIGHT.wide)}</View> : null}

      <View style={styles.choices}>
        {decision.choices.map((choice) => {
          const on = picked?.choice === choice.id;
          const colors = toneColors(theme, choice.requires_reason ? 'warning' : 'accent');
          const tone = { borderColor: colors.fg, backgroundColor: colors.surface };
          return (
            <Pressable key={choice.id} onPress={() => choose(choice)} accessibilityRole="button" accessibilityState={{ selected: on }}
              accessibilityLabel={choice.hint ? `${choice.label}, ${choice.hint}` : choice.label}
              style={({ pressed }) => [styles.choice, on && tone, pressed && !on && styles.pressed]}>
              <Text style={styles.choiceLabel}>{choice.label}</Text>
              {choice.hint ? <Text style={styles.choiceHint}>{choice.hint}</Text> : null}
            </Pressable>
          );
        })}
      </View>

      {option?.requires_reason ? (
        <View style={styles.reasonBox}>
          {option.reason_prompt ? <Text style={styles.reasonPrompt}>{option.reason_prompt}</Text> : null}
          <MultilineInput ref={input} value={picked?.reason ?? ''} onChangeText={(text) => onReason(decision.id, text)}
            accessibilityLabel={option.reason_prompt ?? option.label} maxHeight={140}
            style={[styles.input, { borderColor: reasonMissing ? theme.tokens.status.error : theme.tokens.border.default }]} />
          {reasonMissing ? <Text style={styles.error} accessibilityRole="alert">{m.work_review_reason_required()}</Text> : null}
        </View>
      ) : null}

      <View style={styles.nav}>
        <Pressable onPress={() => onIndex(index - 1)} disabled={index === 0} accessibilityRole="button"
          accessibilityLabel={m.work_review_previous_short()} accessibilityState={{ disabled: index === 0 }}
          style={[styles.navButton, index === 0 && styles.disabled]}>
          <Icon name="ChevronLeft" size={16} color={theme.tokens.text.primary} />
          <Text style={styles.navText}>{m.work_review_previous_short()}</Text>
        </Pressable>
        <Pressable onPress={() => onIndex(index + 1)} disabled={index >= total - 1} accessibilityRole="button"
          accessibilityLabel={m.work_review_next_short()} accessibilityState={{ disabled: index >= total - 1 }}
          style={[styles.navButton, index >= total - 1 && styles.disabled]}>
          <Text style={styles.navText}>{m.work_review_next_short()}</Text>
          <Icon name="ChevronRight" size={16} color={theme.tokens.text.primary} />
        </Pressable>
        <Pressable onPress={onSend} disabled={!ready || sending} accessibilityRole="button" accessibilityLabel={sendLabel}
          accessibilityState={{ disabled: !ready || sending, busy: sending }}
          style={[styles.send, { backgroundColor: theme.tokens.accent.base }, (!ready || sending) && styles.disabled]}>
          {sending ? <ActivityIndicator color={theme.ink.onAccent} /> : null}
          <Text style={styles.sendText}>{sendLabel}</Text>
        </Pressable>
      </View>
      {sendError ? <Text style={styles.error} accessibilityRole="alert">{sendError}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  card: { gap: theme.base.space[2], padding: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.md },
  top: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  tag: { flexShrink: 1, ...theme.type.caption, color: theme.tokens.text.muted },
  position: { marginLeft: 'auto', flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  dot: { width: 7, height: 7, borderRadius: theme.base.radius.full, backgroundColor: theme.tokens.border.strong },
  count: { marginLeft: theme.base.space[1], ...theme.type.caption, color: theme.tokens.text.muted },
  title: { ...theme.type.label, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  reason: { ...theme.type.meta, color: theme.tokens.text.muted },
  reasonStrong: { color: theme.tokens.text.primary, fontWeight: theme.base.weight.medium },
  sides: { flexDirection: 'row', gap: theme.base.space[2] },
  side: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  caption: { flexDirection: 'row', justifyContent: 'space-between', gap: theme.base.space[1] },
  captionLabel: { flexShrink: 1, ...theme.type.caption, color: theme.tokens.text.muted },
  captionValue: { flexShrink: 0, ...theme.type.caption, fontFamily: theme.base.fontMono, color: theme.tokens.text.primary },
  choices: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
  choice: { flexGrow: 1, flexBasis: '40%', minHeight: 44, justifyContent: 'center', gap: theme.base.space[0.5],
    paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1,
    borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.sm },
  pressed: { backgroundColor: theme.tokens.fillSubtle },
  choiceLabel: { ...theme.type.label, color: theme.tokens.text.primary },
  choiceHint: { ...theme.type.caption, color: theme.tokens.text.muted },
  reasonBox: { gap: theme.base.space[1] },
  reasonPrompt: { ...theme.type.meta, color: theme.tokens.text.muted },
  input: { minHeight: 52, borderWidth: 1, borderRadius: theme.base.radius.sm, fontSize: theme.base.text.sm },
  error: { ...theme.type.meta, color: theme.tokens.status.error },
  nav: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[2] },
  navButton: { minHeight: 44, minWidth: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, borderWidth: 1, borderColor: theme.tokens.border.default },
  navText: { ...theme.type.meta, color: theme.tokens.text.primary },
  send: { marginLeft: 'auto', minHeight: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'center',
    gap: theme.base.space[2], paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.sm },
  sendText: { ...theme.type.control, color: theme.ink.onAccent },
  disabled: { opacity: 0.45 },
}));
