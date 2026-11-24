import { useState } from 'react';
import { Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AnswerItem, AskQuestionPayload } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { buildAnswers, type PickState } from './buildAnswers';
import { haptic } from '../../ui/haptics';
import { Icon } from '../../ui/Icon';
import { semEmoji } from '../../ui/semEmoji';
import { superficie } from '../../theme/superficie';

interface Props {
  payload: AskQuestionPayload;
  onSubmit: (answers: AnswerItem[], payload: AskQuestionPayload) => Promise<void>;
  onClose?: (payload: AskQuestionPayload) => void;
}

export function AskStepper(props: Props) {
  const key = JSON.stringify([props.payload.interaction_id, props.payload.request_id, props.payload.questions]);
  return <AskStepperContent key={key} {...props} />;
}

function AskStepperContent({ payload, onSubmit, onClose }: Props) {
  const { theme } = useUnistyles();
  const displayed = { ...payload };
  const questions = payload.questions ?? [];

  const [step, setStep] = useState(0);
  const [picks, setPicks] = useState<PickState[]>(
    () => questions.map(() => ({ kind: 'option', indices: [] }) as PickState),
  );
  const [textOpen, setTextOpen] = useState(false);
  const [textValue, setTextValue] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');

  function advance() {
    setStep((s) => s + 1);
    setTextOpen(false);
    setTextValue('');
  }

  function goBack() {
    setStep((s) => s - 1);
    setTextOpen(false);
    setTextValue('');
  }

  function toggleOption(i: number) {
    const cur = picks[step];
    if (!cur || cur.kind !== 'option') return;
    const q = questions[step];
    if (!q) return;
    haptic.selection();
    if (!q.multiSelect) {
      const next = picks.map((p, idx) => (idx === step ? ({ kind: 'option', indices: [i] } as PickState) : p));
      setPicks(next);
      advance();
    } else {
      const has = cur.indices.includes(i);
      const nextIndices = has ? cur.indices.filter((x) => x !== i) : [...cur.indices, i];
      const next = picks.map((p, idx) => (idx === step ? ({ kind: 'option', indices: nextIndices } as PickState) : p));
      setPicks(next);
    }
  }

  function confirmText() {
    const v = textValue.trim();
    if (!v) return;
    const next = picks.map((p, idx) => (idx === step ? ({ kind: 'text', value: v } as PickState) : p));
    setPicks(next);
    advance();
  }

  function setChat() {
    const next = picks.map((p, idx) => (idx === step ? ({ kind: 'chat' } as PickState) : p));
    setPicks(next);
    advance();
  }

  async function submit() {
    setSending(true);
    setError('');
    try {
      await onSubmit(buildAnswers(questions, picks), displayed);
      haptic.success();
    } catch (e) {
      haptic.error();
      setError(e instanceof Error ? e.message : m.askq_erro_envio());
    } finally {
      setSending(false);
    }
  }

  function pickLabel(qi: number): string {
    const p = picks[qi];
    if (!p) return '—';
    if (p.kind === 'text') return questions[qi]?.isSecret ? '••••••' : p.value;
    if (p.kind === 'chat') return m.askq_conversar();
    const q = questions[qi];
    if (!q) return '—';
    return p.indices.map((i) => q.options[i]?.label ?? '').filter(Boolean).join(', ') || '—';
  }

  const currentPick = picks[step] as PickState | undefined;
  const selectedIndices = currentPick?.kind === 'option' ? currentPick.indices : [];
  const statusHeader = (
    <View style={styles.statusRow}>
      <View style={[styles.statusGlyph, { backgroundColor: superficie(theme, 0.8) }]}>
        <Text style={[styles.statusGlyphText, { color: theme.tokens.accent.base }]}>?</Text>
      </View>
      <Text style={[styles.statusLabel, { color: theme.tokens.text.secondary }]}>{m.board_precisa_de_voce()}</Text>
      <View style={[styles.statusBadge, { backgroundColor: `${theme.tokens.status.warning}20` }]}>
        <Text style={[styles.statusBadgeText, { color: theme.tokens.status.warning }]}>{m.estado_aguardando()}</Text>
      </View>
      {payload.more_after ? (
        <Text style={[styles.counter, { color: theme.tokens.text.muted }]}>{m.askq_more_after({ n: payload.more_after })}</Text>
      ) : null}
    </View>
  );

  // Revisão quando step fora das perguntas
  if (step >= questions.length) {
    return (
      <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
        {statusHeader}
        {/* Escolha única avança sozinha ao tocar: sem esta volta, um toque errado na ÚLTIMA
            pergunta (ou na única) só tinha saída pelo cancelar, que descarta tudo. */}
        <View style={styles.stepNav}>
          <Pressable onPress={goBack} hitSlop={8} disabled={sending} style={styles.backLink} accessibilityRole="button">
            <Icon name="ChevronLeft" size={18} color={theme.tokens.accent.base} />
            <Text style={[styles.backTxt, { color: theme.tokens.accent.base }]}>{m.comum_voltar()}</Text>
          </Pressable>
          <Text style={[styles.counter, { color: theme.tokens.text.muted }]}>{questions.length} / {questions.length}</Text>
        </View>
        <Text style={[styles.sheetTitle, { color: theme.tokens.text.primary }]}>{m.askq_revisar()}</Text>
        <View style={styles.reviewList}>
          {questions.map((q, qi) => (
            <View key={qi} style={[styles.reviewItem, { backgroundColor: superficie(theme), borderColor: theme.tokens.border.subtle }]}>
              <Text style={[styles.reviewQ, { color: theme.tokens.text.secondary }]}>{q.header}</Text>
              <Text style={[styles.reviewA, { color: theme.tokens.text.primary }]}>{pickLabel(qi)}</Text>
            </View>
          ))}
        </View>
        {error ? (
          <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert">
            {error}
          </Text>
        ) : null}
        <Pressable
          onPress={submit}
          disabled={sending}
          style={[styles.primaryBtn, { backgroundColor: theme.tokens.accent.base }, sending && styles.primaryDis]}
          accessibilityRole="button"
        >
          <Text style={styles.primaryTxt}>{sending ? m.askq_enviando() : m.lista_enviar()}</Text>
        </Pressable>
        <Pressable onPress={() => onClose?.(displayed)} style={styles.ghostBtn} accessibilityRole="button">
          <Text style={[styles.ghostTxt, { color: theme.tokens.text.secondary }]}>{m.comum_cancelar()}</Text>
        </Pressable>
      </ScrollView>
    );
  }

  const q = questions[step];

  return (
    <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
      {statusHeader}
      <View style={styles.stepNav}>
        {step > 0 ? (
          <Pressable onPress={goBack} hitSlop={8} style={styles.backLink} accessibilityRole="button">
            <Icon name="ChevronLeft" size={18} color={theme.tokens.accent.base} />
            <Text style={[styles.backTxt, { color: theme.tokens.accent.base }]}>{m.comum_voltar()}</Text>
          </Pressable>
        ) : (
          <View />
        )}
        <Text style={[styles.counter, { color: theme.tokens.text.muted }]}>{step + 1} / {questions.length}</Text>
      </View>

      {/* header chip */}
      {q.header ? (
        <View style={[styles.chip, { backgroundColor: theme.tokens.accent.dim }]}>
          <Text style={[styles.chipTxt, { color: theme.tokens.accent.base }]}>{q.header}</Text>
        </View>
      ) : null}
      <Text style={[styles.question, { color: theme.tokens.text.primary }]}>{q.question}</Text>

      <View style={styles.optionsList}>
        {q.options.map((opt, i) => {
          const sel = selectedIndices.includes(i);
          return (
            <Pressable
              key={i}
              onPress={() => toggleOption(i)}
              style={[
                styles.optionBtn,
                { backgroundColor: superficie(theme), borderColor: sel ? theme.tokens.accent.base : theme.tokens.border.default },
                sel && { backgroundColor: theme.tokens.accent.dim },
              ]}
              accessibilityRole={q.multiSelect ? 'checkbox' : 'radio'}
              accessibilityState={{ checked: sel }}
            >
              <View style={[
                styles.choiceMark,
                { borderColor: theme.tokens.border.strong },
                q.multiSelect && styles.checkBox,
                q.multiSelect && sel && { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.accent.base },
              ]}>
                {q.multiSelect ? (
                  <Text style={[styles.checkMark, { color: sel ? theme.tokens.text.inverse : theme.tokens.accent.base }]}>{sel ? '✓' : ''}</Text>
                ) : sel ? (
                  <View style={[styles.radioDot, { backgroundColor: theme.tokens.accent.base }]} />
                ) : null}
              </View>
              <View style={styles.optContent}>
                <Text style={[styles.optLabel, { color: theme.tokens.text.primary }]}>{opt.label}</Text>
                {opt.description ? (
                  <Text style={[styles.optDesc, { color: theme.tokens.text.secondary }]}>{opt.description}</Text>
                ) : null}
                {opt.preview ? (
                  <View style={[styles.previewBox, { backgroundColor: superficie(theme, 0.8), borderColor: theme.tokens.border.subtle }]}>
                    <Text style={[styles.previewTxt, { color: theme.tokens.text.secondary }]}>{opt.preview}</Text>
                  </View>
                ) : null}
              </View>
            </Pressable>
          );
        })}
      </View>

      {q.multiSelect && !textOpen ? (
        <Pressable
          onPress={advance}
          disabled={selectedIndices.length === 0}
          style={[styles.primaryBtn, { backgroundColor: theme.tokens.accent.base }, selectedIndices.length === 0 && styles.primaryDis]}
          accessibilityRole="button"
        >
          <Text style={styles.primaryTxt}>{m.askq_proximo()}</Text>
        </Pressable>
      ) : null}

      {!textOpen && !(payload.provider === 'codex' && q.options.length === 0) ? (
        payload.provider !== 'codex' || q.isOther ? (
        <View style={[styles.escapes, { borderTopColor: theme.tokens.border.subtle }]}>
          <Pressable
            onPress={() => setTextOpen(true)}
            style={[styles.ghostOpt, { borderColor: theme.tokens.border.default }]}
            accessibilityRole="button"
          >
            <Icon name="PenLine" size={14} color={theme.tokens.text.secondary} />
            <Text style={[styles.ghostOptTxt, { color: theme.tokens.text.primary }]}>{semEmoji(m.askq_digitar_resposta())}</Text>
          </Pressable>
          {payload.provider !== 'codex' ? (
            <Pressable
              onPress={setChat}
              style={[styles.ghostOpt, { borderColor: theme.tokens.border.default }]}
              accessibilityRole="button"
            >
              <Icon name="MessageCircle" size={14} color={theme.tokens.text.secondary} />
              <Text style={[styles.ghostOptTxt, { color: theme.tokens.text.primary }]}>{semEmoji(m.askq_conversar_sobre())}</Text>
            </Pressable>
          ) : null}
        </View>
        ) : null
      ) : (
        <View style={[styles.textEscape, { borderTopColor: theme.tokens.border.subtle }]}>
          <TextInput
            style={[
              styles.fieldInput,
              { backgroundColor: superficie(theme), borderColor: theme.tokens.border.default, color: theme.tokens.text.primary },
            ]}
            value={textValue}
            onChangeText={setTextValue}
            secureTextEntry={q.isSecret}
            autoCorrect={!q.isSecret}
            autoCapitalize={q.isSecret ? 'none' : 'sentences'}
            accessibilityLabel={q.question}
            placeholder={m.askq_sua_resposta()}
            placeholderTextColor={theme.tokens.text.muted}
            autoFocus
          />
          <View style={styles.textActions}>
            <Pressable
              onPress={confirmText}
              disabled={!textValue.trim()}
              style={[styles.primaryBtn, { backgroundColor: theme.tokens.accent.base }, !textValue.trim() && styles.primaryDis]}
              accessibilityRole="button"
            >
              <Text style={styles.primaryTxt}>{m.comum_confirmar()}</Text>
            </Pressable>
            <Pressable
              onPress={() => {
                if (q.options.length === 0) onClose?.(displayed);
                else { setTextOpen(false); setTextValue(''); }
              }}
              style={styles.ghostBtn}
              accessibilityRole="button"
            >
              <Text style={[styles.ghostTxt, { color: theme.tokens.text.secondary }]}>{m.comum_cancelar()}</Text>
            </Pressable>
          </View>
        </View>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  scroll: {
    padding: theme.base.space[4],
    gap: theme.base.space[3],
    paddingBottom: theme.base.space[8],
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  statusGlyph: {
    width: 24,
    height: 24,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: theme.base.radius.full,
  },
  statusGlyphText: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.bold,
  },
  statusLabel: {
    flex: 1,
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  statusBadge: {
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[0.5],
    borderRadius: theme.base.radius.full,
  },
  statusBadgeText: {
    fontSize: theme.base.text.xxxs,
    fontWeight: theme.base.weight.bold,
    textTransform: 'uppercase',
  },
  stepNav: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  backLink: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[0.5],
  },
  backTxt: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.medium,
  },
  counter: {
    fontSize: theme.base.text.sm,
  },
  chip: {
    alignSelf: 'flex-start',
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
  chipTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  sheetTitle: {
    fontSize: theme.base.text.lg,
    fontWeight: theme.base.weight.semibold,
  },
  question: {
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
    lineHeight: 22,
  },
  optionsList: {
    gap: theme.base.space[2],
  },
  optionBtn: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: theme.base.space[3],
    minHeight: 52,
    padding: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
  },
  choiceMark: {
    width: 18,
    height: 18,
    borderWidth: 1.5,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: theme.base.space[0.5],
  },
  checkBox: {
    width: 20,
    height: 20,
    borderRadius: theme.base.radius.xs,
    marginTop: 1,
  },
  radioDot: {
    width: 8,
    height: 8,
    borderRadius: theme.base.radius.full,
  },
  checkMark: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.bold,
    lineHeight: 14,
  },
  optContent: {
    flex: 1,
    gap: theme.base.space[0.5],
  },
  optLabel: {
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  optDesc: {
    fontSize: theme.base.text.sm,
    lineHeight: 18,
  },
  previewBox: {
    marginTop: theme.base.space[2],
    padding: theme.base.space[2],
    borderWidth: 1,
    borderRadius: theme.base.radius.sm,
  },
  previewTxt: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xs,
    lineHeight: 16,
  },
  primaryBtn: {
    height: 50,
    borderRadius: theme.base.radius.md,
    justifyContent: 'center',
    alignItems: 'center',
  },
  primaryDis: {
    opacity: 0.5,
  },
  primaryTxt: {
    color: theme.ink.onAccent,
    fontWeight: theme.base.weight.semibold,
    fontSize: theme.base.text.base,
  },
  ghostBtn: {
    height: 44,
    justifyContent: 'center',
    alignItems: 'center',
    marginTop: theme.base.space[1],
  },
  ghostTxt: {
    fontSize: theme.base.text.sm,
  },
  ghostOpt: {
    height: 44,
    flexDirection: 'row',
    gap: theme.base.space[1.5],
    justifyContent: 'center',
    alignItems: 'center',
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
  },
  ghostOptTxt: {
    fontSize: theme.base.text.sm,
  },
  escapes: {
    gap: theme.base.space[2],
    marginTop: theme.base.space[2],
    borderTopWidth: 1,
    paddingTop: theme.base.space[3],
  },
  textEscape: {
    gap: theme.base.space[3],
    marginTop: theme.base.space[2],
    borderTopWidth: 1,
    paddingTop: theme.base.space[3],
  },
  fieldInput: {
    height: 44,
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    fontSize: theme.base.text.sm,
    paddingHorizontal: theme.base.space[3],
  },
  textActions: {
    gap: theme.base.space[2],
  },
  reviewList: {
    gap: theme.base.space[2],
  },
  reviewItem: {
    padding: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    gap: theme.base.space[0.5],
  },
  reviewQ: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.medium,
  },
  reviewA: {
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  error: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
}));
