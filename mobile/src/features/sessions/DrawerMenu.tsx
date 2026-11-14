import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { listClaudeConfigsForServer, probeServerResponse, rotuloEstado, type AggSession, type CotaContaResumo } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { Icon, type IconName } from '../../ui/Icon';
import { SPACE } from '../../theme/scale';
import { useSessions } from '../../stores/sessions';
import { useServers } from '../../stores/servers';
import { useLiveVoice } from '../voice/liveVoiceStore';
import { accountQuotaLine, recentSessions } from './drawerMenu';
import { HeadlessSignal } from './HeadlessSignal';
import { SessionAvatar } from './SessionAvatar';

const RECENT = 5;
const NO_ROWS: AggSession[] = [];

// Menu da gaveta no padrão dos apps de chat: destinos, recentes compactos e a ação principal
// flutuando embaixo. A lista completa de sessões mora no Início. `onClose` roda antes de navegar.
export function DrawerMenu({ open, onClose, onOpenServers }: { open: boolean; onClose: () => void; onOpenServers: () => void }) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  // Fechada, a gaveta continua montada: sem assinar a lista, troca de estado das sessões não a redesenha.
  const rows = useSessions((s) => (open ? s.rows : NO_ROWS));
  const recent = useMemo(() => recentSessions(rows, RECENT), [rows]);
  const server = useServers((s) => s.servers.find((x) => x.id === s.activeId));
  const activeId = server?.id;
  const voiceOk = !!server && !server.invite && !server.inviteEnded && !server.removed;
  const quota = useAccountQuota(server?.invite ? undefined : server, open);

  const closeRef = useRef(onClose);
  useLayoutEffect(() => { closeRef.current = onClose; }, [onClose]);
  const go = useCallback((path: string) => { closeRef.current(); router.push(path as never); }, [router]);
  const openSession = useCallback((session: AggSession) => go(`/s/${session.serverId}/${session.name}`), [go]);
  const ink = theme.tokens.text.primary;
  const item = (icon: IconName, label: string, onPress: () => void, hint?: string) => (
    <Pressable key={label} onPress={onPress} accessibilityRole="button" accessibilityLabel={hint ? `${label}, ${hint}` : label}
      style={({ pressed }) => [styles.item, pressed && styles.pressed]}>
      <Icon name={icon} size={20} color={ink} />
      <Text style={styles.itemText} numberOfLines={1}>{label}</Text>
      {hint ? <Text style={styles.hint} numberOfLines={1}>{hint}</Text> : null}
    </Pressable>
  );

  return (
    <View style={[styles.root, { paddingTop: insets.top }]}>
      <View style={styles.head}>
        <Text style={styles.brand} accessibilityRole="header" numberOfLines={1}>{m.native_brand()}</Text>
        <Pressable onPress={() => go('/search')} style={styles.circle} hitSlop={4}
          accessibilityRole="button" accessibilityLabel={m.switcher_buscar_conversas()}>
          <Icon name="Search" size={19} color={ink} />
        </Pressable>
      </View>
      <ScrollView contentContainerStyle={{ paddingBottom: insets.bottom + 96 }} showsVerticalScrollIndicator={false}>
        {item('House', m.menu_inicio(), onClose)}
        {item('ChartNoAxesCombined', m.nav_custos(), () => go('/costs'))}
        {voiceOk && activeId ? item('AudioLines', m.voz_titulo(), () => { onClose(); useLiveVoice.getState().openFor(activeId, ''); }) : null}
        {item('Server', m.maquinas_titulo(), () => { onClose(); onOpenServers(); })}
        {item('CircleUser', m.contas_titulo(), () => go('/config/contas'), quota || undefined)}
        {item('GitBranch', m.worktrees_titulo(), () => go('/config/worktrees'))}
        {item('GitPullRequest', m.native_review_title(), () => go('/reviews'))}
        {recent.length ? <Text style={styles.section} accessibilityRole="header">{m.conversas_recentes()}</Text> : null}
        {recent.map((s) => <RecentRow key={`${s.serverId}:${s.name}`} session={s} onOpen={openSession} />)}
        <Pressable onPress={onClose} accessibilityRole="button" style={({ pressed }) => [styles.item, pressed && styles.pressed]}>
          <Text style={styles.more}>{m.native_sidebar_all_sessions()}</Text>
        </Pressable>
      </ScrollView>
      <View style={[styles.bottom, { bottom: insets.bottom + 12 }]} pointerEvents="box-none">
        <Pressable onPress={() => go('/create')} accessibilityRole="button" accessibilityLabel={m.sessao_nova()}
          style={({ pressed }) => [styles.pill, pressed && styles.pillPressed]}>
          <Icon name="Plus" size={18} color={theme.tokens.bg.base} />
          <Text style={styles.pillText}>{m.sessao_nova()}</Text>
        </Pressable>
        <Pressable onPress={() => go('/config')} style={styles.circle} hitSlop={4}
          accessibilityRole="button" accessibilityLabel={m.config_modal_titulo()}>
          <Icon name="Settings" size={20} color={ink} />
        </Pressable>
      </View>
    </View>
  );
}

const RecentRow = memo(function RecentRow({ session, onOpen }: { session: AggSession; onOpen: (session: AggSession) => void }) {
  const { theme } = useUnistyles();
  return (
    <Pressable onPress={() => onOpen(session)} accessibilityRole="button" accessibilityLabel={`${session.name}${session.headless === true ? `, ${m.criar_modo_exec_headless()}` : ''}, ${rotuloEstado(session.state)}, ${session.serverLabel}`}
      style={({ pressed }) => [styles.item, styles.recent, pressed && styles.pressed]}>
      <SessionAvatar session={session} size={SPACE[8]} />
      <View style={styles.recentText}>
        <View style={styles.recentLine}>
          <Text style={styles.itemText} numberOfLines={1}>{session.name}</Text>
          {session.headless === true ? <HeadlessSignal /> : null}
        </View>
        <View style={styles.recentLine} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          <Icon name="Monitor" size={SPACE[3]} color={theme.tokens.text.muted} />
          <Text style={styles.serverLabel} numberOfLines={1}>{session.serverLabel}</Text>
        </View>
      </View>
    </Pressable>
  );
});

// A cota é só uma dica na linha: sem leitura, a linha fica sem ela e a tela de Contas mostra o motivo.
// Relida a cada abertura: a gaveta fica montada e o número de horas atrás pareceria atual.
function useAccountQuota(server: Parameters<typeof listClaudeConfigsForServer>[0] | undefined, open: boolean): string {
  const [line, setLine] = useState('');
  useEffect(() => {
    if (!open) return;
    if (!server) { setLine(''); return; }
    let alive = true;
    const cotas = probeServerResponse(server, '/api/cotas')
      .then((r) => (r.ok ? r.json() as Promise<CotaContaResumo[]> : []))
      .catch(() => [] as CotaContaResumo[]);
    void Promise.all([listClaudeConfigsForServer(server), cotas])
      .then(([configs, cs]) => { if (alive) setLine(accountQuotaLine(configs, cs)); })
      .catch(() => { if (alive) setLine(''); });
    return () => { alive = false; };
  }, [server, open]);
  return line;
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1 },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingLeft: theme.base.space[5], paddingRight: theme.base.space[3], paddingVertical: theme.base.space[2] },
  brand: { flex: 1, fontSize: theme.base.text.xl, fontWeight: theme.base.weight.bold, color: theme.tokens.text.primary },
  circle: { width: 44, height: 44, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center', backgroundColor: theme.tokens.bg.hover },
  item: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[5] },
  pressed: { backgroundColor: theme.tokens.bg.hover },
  itemText: { flexShrink: 1, fontSize: theme.base.text.base, color: theme.tokens.text.primary },
  hint: { flex: 1, textAlign: 'right', fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  section: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[5], paddingTop: theme.base.space[4], paddingBottom: theme.base.space[1] },
  more: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
  recent: { minHeight: 48, paddingVertical: theme.base.space[1] },
  recentText: { flex: 1, minWidth: 0 },
  recentLine: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  serverLabel: { ...theme.type.caption, flexShrink: 1, color: theme.tokens.text.muted },
  bottom: { position: 'absolute', left: 16, right: 16, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' },
  pill: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[4], borderRadius: theme.base.radius.full, backgroundColor: theme.tokens.text.primary, boxShadow: '0px 6px 20px rgba(0,0,0,0.35)' },
  pillPressed: { opacity: 0.85 },
  pillText: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, color: theme.tokens.bg.base },
}));
