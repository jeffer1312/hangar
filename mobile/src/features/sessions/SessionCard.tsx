import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { relativeTime, providerTag, untrackedReason } from '@hangar/core';
import type { AggSession } from '@hangar/core';
import { StatePill } from './StatePill';

// Porte mínimo de SessionCard.svelte para o celular: nome, provider, pílula,
// label/question, branch, tempo relativo. O resto da Svelte (swipe, diff, loop,
// etc) não entra aqui — Task 6 é lista ao vivo, não detalhamento.

export function SessionCard({
  session,
  onPress,
}: {
  session: AggSession;
  onPress: () => void;
}) {
  const isUntracked = session.tracked === false;
  // kimi sem id é estado NORMAL pré-1º prompt — não bloqueia abrir
  const canOpen = !isUntracked || session.provider === 'kimi';
  const tag = providerTag(session.provider);
  const ago = relativeTime(session.last_activity);
  // cwd: mostra só quando acrescenta — basename != nome
  const cwdBase = (() => {
    const p = (session.cwd ?? '').replace(/\/+$/, '');
    const i = p.lastIndexOf('/');
    return i >= 0 ? p.slice(i + 1) : p;
  })();
  const showCwd = !!session.cwd && cwdBase.toLowerCase() !== session.name.toLowerCase();
  const branch = session.branch ?? null;
  const sub = session.question ?? session.label ?? null;
  const subIsQuestion = !!session.question;

  return (
    <Pressable
      onPress={onPress}
      disabled={!canOpen}
      style={({ pressed }) => [styles.card, pressed && styles.pressed]}
      accessibilityRole="button"
      accessibilityLabel={session.name}
    >
      <View style={styles.head}>
        <Text style={styles.name} numberOfLines={1}>
          {session.name}
        </Text>
        {tag ? (
          <View style={styles.prov}>
            <Text style={styles.provTxt}>{tag}</Text>
          </View>
        ) : null}
        {isUntracked ? (
          <View style={styles.untracked}>
            <Text style={styles.untrackedTxt}>sem id</Text>
          </View>
        ) : null}
        <View style={styles.spacer} />
        <StatePill state={session.state} />
      </View>

      {sub ? (
        <Text style={[styles.sub, subIsQuestion ? styles.subAsk : styles.subWork]} numberOfLines={1}>
          {sub}
        </Text>
      ) : null}

      <View style={styles.meta}>
        {showCwd ? (
          <Text style={styles.cwd} numberOfLines={1}>
            {cwdBase}
          </Text>
        ) : null}
        {showCwd && branch ? <Text style={styles.sep}>·</Text> : null}
        {branch ? (
          <Text style={styles.branch} numberOfLines={1}>
            {branch}
          </Text>
        ) : null}
        {(showCwd || branch) && ago ? <Text style={styles.sep}>·</Text> : null}
        {ago ? <Text style={styles.ago}>{ago}</Text> : null}
      </View>

      {isUntracked && session.provider !== 'kimi' ? (
        <Text style={styles.hint} numberOfLines={2}>
          {untrackedReason(session.provider)}
        </Text>
      ) : null}
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  card: {
    backgroundColor: theme.tokens.bg.elevated,
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    padding: theme.base.space[3],
    gap: 6,
    minHeight: 44,
  },
  pressed: {
    opacity: 0.7,
  },
  head: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    minWidth: 0,
  },
  name: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
    color: theme.tokens.text.primary,
    flexShrink: 1,
  },
  prov: {
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 9999,
    backgroundColor: theme.tokens.bg.hover,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
  },
  provTxt: {
    fontSize: 10,
    fontWeight: '700',
    color: theme.tokens.text.muted,
    letterSpacing: 0.2,
  },
  untracked: {
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 9999,
    borderWidth: 1,
    borderColor: theme.tokens.status.warning,
  },
  untrackedTxt: {
    fontSize: 10,
    fontWeight: '600',
    color: theme.tokens.status.warning,
  },
  spacer: { flex: 1 },
  sub: {
    fontSize: theme.base.text.xs,
    minWidth: 0,
  },
  subAsk: { color: theme.tokens.status.warning, fontWeight: '600' },
  subWork: { color: theme.tokens.text.secondary, fontStyle: 'italic' },
  meta: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
  },
  cwd: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.text.secondary,
    fontFamily: theme.base.fontMono,
    flexShrink: 1,
  },
  sep: { color: theme.tokens.text.muted, fontSize: theme.base.text.xs },
  branch: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.accent.base,
    fontFamily: theme.base.fontMono,
    flexShrink: 1,
  },
  ago: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.text.muted,
    flexShrink: 0,
  },
  hint: {
    fontSize: 11,
    color: theme.tokens.text.muted,
    marginTop: 2,
  },
}));
