import { useState } from 'react';
import { Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AnswerItem, AskQuestionPayload } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { buildAnswers, type PickState } from './buildAnswers';

interface Props {
  payload: AskQuestionPayload;
  onSubmit: (answers: AnswerItem[]) => Promise<void>;
  onClose?: () => void;
}

export function AskStepper({ payload, onSubmit, onClose }: Props) {
  const { theme } = useUnistyles();
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
      await onSubmit(buildAnswers(questions, picks));
    } catch (e) {
      setError(e instanceof Error ? e.message : m.askq_erro_envio());
    } finally {
      setSending(false);
    }
  }

  function pickLabel(qi: number): string {
    const p = picks[qi];
    if (!p) return '—';
    if (p.kind === 'text') return p.value;
    if (p.kind === 'chat') return m.askq_conversar();
    const q = questions[qi];
    if (!q) return '—';
    return p.indices.map((i) => q.options[i]?.label ?? '').filter(Boolean).join(', ') || '—';
  }

  const currentPick = picks[step] as PickState | undefined;
  const selectedIndices = currentPick?.kind === 'option' ? currentPick.indices : [];

  // Revisão quando step fora das perguntas
  if (step >= questions.length) {
    return (
      <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
        <Text style={[styles.sheetTitle, { color: theme.tokens.text.primary }]}>{m.askq_revisar()}</Text>
        <View style={styles.reviewList}>
          {questions.map((q, qi) => (
            <View key={qi} style={[styles.reviewItem, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}>
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
        <Pressable onPress={onClose} style={styles.ghostBtn} accessibilityRole="button">
          <Text style={[styles.ghostTxt, { color: theme.tokens.text.secondary }]}>{m.comum_cancelar()}</Text>
        </Pressable>
      </ScrollView>
    );
  }

  const q = questions[step];

  return (
    <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
      <View style={styles.stepNav}>
        {step > 0 ? (
          <Pressable onPress={goBack} hitSlop={8} style={styles.backLink} accessibilityRole="button">
            <Text style={[styles.backTxt, { color: theme.tokens.accent.base }]}>{'‹ '}{m.comum_voltar()}</Text>
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
                { backgroundColor: theme.tokens.bg.surface, borderColor: sel ? theme.tokens.accent.base : theme.tokens.border.default },
                sel && { backgroundColor: theme.tokens.accent.dim },
              ]}
              accessibilityRole="button"
            >
              {q.multiSelect ? (
                <View style={[styles.checkBox, { borderColor: theme.tokens.border.strong }, sel && { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.accent.base }]}>
                  <Text style={[styles.checkMark, { color: sel ? '#fff' : theme.tokens.accent.base }]}>{sel ? '✓' : ''}</Text>
                </View>
              ) : null}
              <View style={styles.optContent}>
                <Text style={[styles.optLabel, { color: theme.tokens.text.primary }]}>{opt.label}</Text>
                {opt.description ? (
                  <Text style={[styles.optDesc, { color: theme.tokens.text.secondary }]}>{opt.description}</Text>
                ) : null}
                {opt.preview ? (
                  <View style={[styles.previewBox, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}>
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

      {!textOpen ? (
        <View style={[styles.escapes, { borderTopColor: theme.tokens.border.subtle }]}>
          <Pressable
            onPress={() => setTextOpen(true)}
            style={[styles.ghostOpt, { borderColor: theme.tokens.border.default }]}
            accessibilityRole="button"
          >
            <Text style={[styles.ghostOptTxt, { color: theme.tokens.text.primary }]}>{m.askq_digitar_resposta()}</Text>
          </Pressable>
          <Pressable
            onPress={setChat}
            style={[styles.ghostOpt, { borderColor: theme.tokens.border.default }]}
            accessibilityRole="button"
          >
            <Text style={[styles.ghostOptTxt, { color: theme.tokens.text.primary }]}>{m.askq_conversar_sobre()}</Text>
          </Pressable>
        </View>
      ) : (
        <View style={[styles.textEscape, { borderTopColor: theme.tokens.border.subtle }]}>
          <TextInput
            style={[
              styles.fieldInput,
              { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.default, color: theme.tokens.text.primary },
            ]}
            value={textValue}
            onChangeText={setTextValue}
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
                setTextOpen(false);
                setTextValue('');
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
    paddingBottom: 32,
  },
  stepNav: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  backLink: {
    minHeight: 44,
    justifyContent: 'center',
  },
  backTxt: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  counter: {
    fontSize: theme.base.text.sm,
  },
  chip: {
    alignSelf: 'flex-start',
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 4,
  },
  chipTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  sheetTitle: {
    fontSize: 20,
    fontWeight: '600',
  },
  question: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
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
  checkBox: {
    width: 22,
    height: 22,
    borderWidth: 1.5,
    borderRadius: 4,
    alignItems: 'center',
    justifyContent: 'center',
  },
  checkMark: {
    fontSize: 12,
    fontWeight: '700',
    lineHeight: 14,
  },
  optContent: {
    flex: 1,
    gap: 2,
  },
  optLabel: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
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
    color: '#fff',
    fontWeight: '600',
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
    fontSize: 16,
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
    gap: 2,
  },
  reviewQ: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  reviewA: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
  },
  error: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
}));
