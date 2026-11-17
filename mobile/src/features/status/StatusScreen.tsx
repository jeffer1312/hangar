import { useCallback, useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { Platform, ScrollView, Text, View } from 'react-native';
import { Stack } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import * as Clipboard from 'expo-clipboard';
import { useUnistyles } from 'react-native-unistyles';
import type { StatusAttentionTarget, StatusSnapshot } from '@hangar/core';
import { Pagina } from '../config/Pagina';
import { InfoNotice } from '../config/InfoNotice';
import { useSettingsColors, withAlpha } from '../config/colors';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { RADIUS, SPACE } from '../../theme/scale';
import { ActionButton } from './StatusParts';
import { DaysCard, HealthCard, HostCard, MachinesCard, OutsideCard, ServerTiles, SessionsCard, SlowCard } from './StatusBlocks';
import { duration } from './format';
import { useStatus, type StatusFailure } from './useStatus';
import * as m from '../../paraglide/messages';

export default function StatusScreen() {
  const { view, retry } = useStatus();
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const scroll = useRef<ScrollView | null>(null);
  return <Pagina scrollRef={scroll}>
    <Stack.Screen options={{ title: m.status_title() }} />
    {view.kind === 'none' ? <InfoNotice text={m.maquinas_vazio()} /> : <>
      <Text numberOfLines={1} style={{ color: c.faint, fontSize: theme.base.text.xs, marginBottom: -SPACE[2] }}>{view.label}</Text>
      {view.kind === 'loading' ? <Loading /> : view.kind === 'down' ? <Down label={view.label} since={view.since} onRetry={retry} />
        : view.kind === 'error' ? <Failed label={view.label} failure={view.failure} onRetry={retry} /> : <Body snap={view.snap} scroll={scroll} />}
    </>}
  </Pagina>;
}

function Body({ snap, scroll }: { snap: StatusSnapshot; scroll: RefObject<ScrollView | null> }) {
  const header = useHeaderHeight();
  const places = useRef<Partial<Record<StatusAttentionTarget, number>>>({});
  const [flash, setFlash] = useState<StatusAttentionTarget | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  // Uma função por alvo, a mesma em todo retrato: os cartões em `memo` não redesenham por ela.
  const placers = useRef<Partial<Record<StatusAttentionTarget, (y: number) => void>>>({});
  const place = (target: StatusAttentionTarget) => (placers.current[target] ??= (y: number) => { places.current[target] = y; });
  const jump = useCallback((target: StatusAttentionTarget) => {
    const y = places.current[target];
    if (y === undefined) return;
    // No iOS o recuo do cabeçalho é do próprio ScrollView: o deslocamento zero fica embaixo dele.
    const top = Platform.OS === 'ios' ? header : 0;
    scroll.current?.scrollTo({ y: Math.max(-top, y - top - SPACE[3]), animated: true });
    setFlash(target);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setFlash(null), 1200);
  }, [header, scroll]);
  const at = (target: StatusAttentionTarget) => ({ flash: flash === target, onLayout: place(target) });
  return <>
    <HealthCard attention={snap.attention} server={snap.server} onJump={jump} />
    {snap.server ? <ServerTiles server={snap.server} onLayout={place('server')} /> : null}
    {snap.exits || snap.update ? <DaysCard exits={snap.exits} update={snap.update} {...at('exits')} /> : null}
    {snap.machines ? <MachinesCard machines={snap.machines} {...at('machines')} /> : null}
    {snap.sessions ? <SessionsCard sessions={snap.sessions} {...at('sessions')} /> : null}
    {snap.slow ? <SlowCard slow={snap.slow} {...at('slow')} /> : null}
    {snap.host ? <HostCard host={snap.host} {...at('host')} /> : null}
    {snap.outside || snap.leftovers ? <OutsideCard outside={snap.outside} leftovers={snap.leftovers} {...at('outside')} /> : null}
  </>;
}

function Loading() {
  const c = useSettingsColors();
  const bone = (h: number, w: number | `${number}%` = '100%') => <View style={{ height: h, width: w, borderRadius: RADIUS.sm, backgroundColor: c.hover }} />;
  const card = (h: number, key: number) => <View key={key} style={{ padding: SPACE[4], gap: SPACE[3], borderRadius: RADIUS.lg, backgroundColor: c.inset }}>{bone(12, 120)}{bone(h)}</View>;
  return <View accessible accessibilityRole="progressbar" accessibilityLabel={m.status_loading()} style={{ gap: SPACE[4] }}>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[3], padding: SPACE[4], borderRadius: RADIUS.lg, backgroundColor: c.inset }}>
      <View style={{ width: 52, height: 52, borderRadius: RADIUS.full, backgroundColor: c.hover }} />
      <View style={{ flex: 1, gap: SPACE[2] }}>{bone(16, 160)}{bone(11, 200)}</View>
    </View>
    <View style={{ flexDirection: 'row', gap: SPACE[2] }}>
      <View style={{ flex: 1 }}>{card(60, 0)}</View>
      <View style={{ flex: 1 }}>{card(60, 1)}</View>
    </View>
    {[58, 110].map(card)}
  </View>;
}

/** Cartão grande de estado: ícone na cor do tom, título, texto e as ações. */
function StateCard({ tone, icon, title, body, children }: { tone: string; icon: 'TriangleAlert' | 'X'; title: string; body: string; children: ReactNode }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View accessibilityRole="alert" style={{ padding: SPACE[4], gap: SPACE[3], borderRadius: RADIUS.lg, backgroundColor: c.inset }}>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[3] }}>
      <View style={{ width: 52, height: 52, borderRadius: RADIUS.full, alignItems: 'center', justifyContent: 'center', backgroundColor: withAlpha(tone, 0.18) }}>
        <Icon name={icon} size={24} color={tone} />
      </View>
      <Text accessibilityRole="header" style={{ flex: 1, color: c.text, ...theme.type.heading }}>{title}</Text>
    </View>
    <Text style={{ color: c.muted, fontSize: theme.base.text.sm }}>{body}</Text>
    <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: SPACE[2] }}>{children}</View>
  </View>;
}

function Failed({ label, failure, onRetry }: { label: string; failure: StatusFailure; onRetry: () => void }) {
  const { theme } = useUnistyles();
  // O 409 de identidade vem depois de a rota ser decidida: é o endereço que não provou ser a máquina, não um HTTP do servidor.
  const body = failure.reason === 'server_identity_unconfirmed' ? m.mobile_machines_identity_unconfirmed()
    : failure.code === 404 ? m.status_error_outdated({ server: label })
    : failure.code ? m.status_error_http({ server: label, code: String(failure.code) }) : m.status_error_other({ server: label });
  const copy = () => { void Clipboard.setStringAsync(`${label} /api/status: ${failure.detail}`).then(() => toast.ok(m.status_copied())); };
  return <StateCard tone={theme.tokens.status.warning} icon="TriangleAlert" title={m.status_error_title()} body={body}>
    <ActionButton icon="RefreshCw" label={m.status_retry()} onPress={onRetry} />
    <ActionButton icon="Copy" label={m.status_copy_detail()} onPress={copy} />
  </StateCard>;
}

/** Servidor parado: o celular não sobe nada, só diz onde resolver. */
function Down({ label, since, onRetry }: { label: string; since?: number; onRetry: () => void }) {
  const { theme } = useUnistyles();
  const body = since !== undefined ? m.status_down_since({ time: duration(Date.now() - since) }) : m.status_down_body();
  return <StateCard tone={theme.tokens.status.error} icon="X" title={m.status_down_title({ server: label })} body={body}>
    <ActionButton icon="RefreshCw" label={m.status_retry()} onPress={onRetry} />
  </StateCard>;
}
