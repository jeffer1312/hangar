import { useMemo, useRef, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import ReanimatedSwipeable, { type SwipeableMethods } from 'react-native-gesture-handler/ReanimatedSwipeable';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Haptics from 'expo-haptics';
import { cwdParts, loopBadge, planBadge, providerTag, relativeTime, rotuloEstado, untrackedReason, type AggSession } from '@hangar/core';
import { Chip, type Tone } from '../../ui/Chip';
import { StateDot } from '../../ui/StateDot';
import { Icon } from '../../ui/Icon';
import { PlanBar } from '../plan/PlanBar';
import { SessionMenu } from './SessionMenu';
import * as m from '../../paraglide/messages';

// LOOP_TONE_COLOR do core é CSS var (`var(--accent)`) — não serve em RN; o tom vira `Chip tone`.
const TOM_DO_LOOP: Record<'ok' | 'warn' | 'attention' | 'muted', Tone> = {
  ok: 'success',
  warn: 'warning',
  attention: 'error',
  muted: 'neutral',
};

interface Props {
  session: AggSession;
  mostrarServidor: boolean; // false quando a lista já está agrupada por servidor
  onPress: () => void;
  onGit: () => void;
  onExcluir: () => void;
  onRenomear: () => void;
  onLoop: () => void;
  onResume: () => void;
  // a lista fecha a linha aberta anterior quando esta abre (uma aberta por vez)
  aoAbrir?: (metodos: SwipeableMethods | null) => void;
}

export function SessionRow({ session: s, mostrarServidor, onPress, onGit, onExcluir, onRenomear, onLoop, onResume, aoAbrir }: Props) {
  const { theme } = useUnistyles();
  const swipe = useRef<SwipeableMethods>(null);
  const [menuAberto, setMenuAberto] = useState(false);
  // Toque longo pelo gesture-handler, não pelo `Pressable`: assim ele convive com o arrasto do
  // swipe (o mesmo reconhecedor decide quem ganha) em vez de disputar o toque com ele.
  const toqueLongo = useMemo(
    () =>
      Gesture.LongPress()
        .minDuration(500)
        .runOnJS(true)
        .onStart(() => {
          Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
          setMenuAberto(true);
        }),
    [],
  );
  // Renomear/Git/Loop/Excluir só existiam no toque longo e no arrasto — dois gestos que o leitor
  // de tela não alcança. Aqui elas viram ações do rotor/menu de acessibilidade da própria linha.
  const acoesA11y = useMemo(
    () => [
      { name: 'rename', label: m.sessao_renomear() },
      ...(s.cwd ? [{ name: 'git', label: 'Git' }] : []),
      { name: 'loop', label: m.loop_titulo() },
      { name: 'delete', label: m.sessao_excluir_curto() },
    ],
    [s.cwd],
  );
  const untracked = s.tracked === false;
  const cwd = cwdParts(s.cwd);
  const showCwd = !!s.cwd && cwd.base.toLowerCase() !== s.name.toLowerCase();
  const loop = loopBadge(s.loop_status, s.loop_iter, s.loop_max);
  const plan = planBadge(s);
  const sub = s.question ?? (s.state === 'working' ? s.label : null) ?? null;

  const acoes = () => (
    <View style={styles.acoes}>
      {s.cwd ? (
        <Pressable onPress={onGit} style={[styles.acao, { backgroundColor: theme.tokens.accent.base }]} accessibilityRole="button" accessibilityLabel="Git">
          <Icon name="GitBranch" size={18} color="#fff" />
        </Pressable>
      ) : null}
      <Pressable onPress={onExcluir} style={[styles.acao, { backgroundColor: theme.tokens.status.error }]} accessibilityRole="button" accessibilityLabel={m.sessao_excluir_curto()}>
        <Icon name="Trash2" size={18} color="#fff" />
      </Pressable>
    </View>
  );

  return (
    <ReanimatedSwipeable
      ref={swipe}
      renderRightActions={acoes}
      rightThreshold={40}
      overshootRight={false}
      onSwipeableWillOpen={() => aoAbrir?.(swipe.current)}
      simultaneousWithExternalGesture={toqueLongo}
    >
      <SessionMenu
        aberto={menuAberto}
        onFechar={() => setMenuAberto(false)}
        temCwd={!!s.cwd}
        onRenomear={onRenomear}
        onGit={onGit}
        onLoop={onLoop}
        onExcluir={onExcluir}
      >
      <GestureDetector gesture={toqueLongo}>
        <Pressable
          onPress={onPress}
          // sem onLongPress: o toque longo é do `toqueLongo` acima, que abre a folha de ações
          // kimi sem id é estado NORMAL pré-1º prompt — não bloqueia abrir
          disabled={untracked && s.provider !== 'kimi'}
          style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.tokens.bg.hover }]}
          accessibilityRole="button"
          // rótulo composto: um label explícito no pai faz o RN descartar o texto dos filhos, e o
          // estado e a pergunta sumiriam do leitor de tela.
          accessibilityLabel={`${s.name}, ${rotuloEstado(s.state)}${sub ? `, ${sub}` : ''}`}
          accessibilityActions={acoesA11y}
          onAccessibilityAction={({ nativeEvent }) => {
            if (nativeEvent.actionName === 'rename') onRenomear();
            else if (nativeEvent.actionName === 'git') onGit();
            else if (nativeEvent.actionName === 'loop') onLoop();
            else if (nativeEvent.actionName === 'delete') onExcluir();
          }}
        >
          <View style={styles.lead}><StateDot state={s.state} /></View>
          <View style={styles.col}>
            <View style={styles.linha1}>
              <Text style={[styles.nome, { color: theme.tokens.text.primary }]} numberOfLines={1}>{s.name}</Text>
              {providerTag(s.provider) ? <Chip>{providerTag(s.provider)!}</Chip> : null}
              {untracked ? <Chip tone="warning">{m.sessao_sem_id()}</Chip> : null}
            </View>
            {sub ? (
              <Text
                style={[styles.sub, { color: s.question ? theme.tokens.status.warning : theme.tokens.text.secondary, fontStyle: s.question ? 'normal' : 'italic' }]}
                numberOfLines={1}
              >
                {sub}
              </Text>
            ) : null}
            <View style={styles.meta}>
              {mostrarServidor ? <Text style={[styles.metaTxt, { color: s.serverColor }]} numberOfLines={1}>{s.serverLabel}</Text> : null}
              {s.worktree ? <Chip mono>worktree</Chip> : null}
              {s.branch ? <Text style={[styles.metaTxt, styles.mono, { color: theme.tokens.accent.base }]} numberOfLines={1}>⎇ {s.branch}</Text> : null}
              {s.git_added || s.git_removed ? (
                <Text style={[styles.metaTxt, styles.mono]}>
                  {s.git_added ? <Text style={{ color: theme.tokens.status.success }}>+{s.git_added}</Text> : null}
                  {s.git_removed ? <Text style={{ color: theme.tokens.status.error }}> −{s.git_removed}</Text> : null}
                </Text>
              ) : null}
              {showCwd ? (
                <Text style={[styles.metaTxt, styles.mono, { color: theme.tokens.text.secondary, flexShrink: 1 }]} numberOfLines={1}>
                  {cwd.prefix}{cwd.base}
                </Text>
              ) : null}
              <Text style={[styles.metaTxt, { color: theme.tokens.text.muted, marginLeft: 'auto' }]}>{relativeTime(s.last_activity)}</Text>
            </View>
            {s.pair_peers?.length || s.limited || loop || plan || s.engine ? (
              <View style={styles.chips}>
                {s.pair_peers?.length ? <Chip icon="Users">{s.pair_peers.length === 1 ? s.pair_peers[0] : String(s.pair_peers.length + 1)}</Chip> : null}
                {s.limited ? <Chip tone="warning" icon="Hourglass">{s.limit_reset ?? ''}</Chip> : null}
                {loop ? <Chip tone={TOM_DO_LOOP[loop.tone]}>{loop.label}</Chip> : null}
                {plan ? <Chip tone={plan.complete ? 'success' : 'accent'} icon="ClipboardList">{plan.label}</Chip> : null}
                {s.engine ? <Chip icon="Cog">{s.engine}</Chip> : null}
              </View>
            ) : null}
            {/* não `compact`: ali a barra é absoluta no rodapé da coluna e atravessa o chip do plano */}
            {plan ? <PlanBar session={s} /> : null}
            {untracked && s.provider !== 'kimi' && s.provider !== 'pi' && s.provider !== 'omp' ? (
              <Pressable onPress={onResume} style={styles.resume} accessibilityRole="button" accessibilityLabel={m.sessao_retomar()}>
                <Text style={{ color: theme.tokens.accent.base, fontSize: theme.base.text.xs }}>↻ {m.sessao_retomar()}</Text>
              </Pressable>
            ) : untracked ? (
              <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>{untrackedReason(s.provider)}</Text>
            ) : null}
          </View>
          <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} />
        </Pressable>
      </GestureDetector>
      </SessionMenu>
    </ReanimatedSwipeable>
  );
}

const styles = StyleSheet.create((theme) => ({
  row: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingVertical: 10, paddingHorizontal: 12, minHeight: 56 },
  lead: { width: 12, alignItems: 'center' },
  col: { flex: 1, gap: 3, minWidth: 0 },
  linha1: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  nome: { fontSize: theme.base.text.base, fontWeight: '600', flexShrink: 1 },
  sub: { fontSize: theme.base.text.xs },
  meta: { flexDirection: 'row', alignItems: 'center', gap: 6, flexWrap: 'nowrap' },
  metaTxt: { fontSize: theme.base.text.xxs },
  mono: { fontFamily: theme.base.fontMono },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: 4, marginTop: 2 },
  resume: { marginTop: 2 },
  acoes: { flexDirection: 'row' },
  acao: { width: 64, justifyContent: 'center', alignItems: 'center' },
}));
