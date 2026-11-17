import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { intlLocale, type StatusAttention, type StatusAttentionTarget, type StatusExit, type StatusExitsBlock, type StatusHostBlock,
  type StatusLeftoversBlock, type StatusMachine, type StatusOutsideBlock, type StatusServerBlock, type StatusSessionsBlock,
  type StatusSlowBlock, type StatusUpdateBlock, type StatusUsage } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { Segmented } from '../usage/CostsVisuals';
import { Composition, useChartColors } from '../usage/ReportUI';
import { useSettingsColors, withAlpha } from '../config/colors';
import { RADIUS, SPACE } from '../../theme/scale';
import { Block, Chip, Chips, HourColumns, Legend, Meter, Mono, SegBar, Signal, Sparkline, type HourColumn } from './StatusParts';
import { attentionText, bytes, duration, exitDetail, exitKindText, num, pct, platformName, processName, providerLabel, since, updateLines, when } from './format';
import * as m from '../../paraglide/messages';

type Place = { flash?: boolean; onLayout?: (y: number) => void };

export function HealthCard({ attention, server, onJump }: { attention: StatusAttention[]; server?: StatusServerBlock; onJump: (target: StatusAttentionTarget) => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const ok = attention.length === 0;
  const tone = ok ? theme.tokens.status.success : theme.tokens.status.warning;
  const title = ok ? m.status_health_ok() : attention.length === 1 ? m.status_health_attention_one() : m.status_health_attention({ count: String(attention.length) });
  return <View style={{ padding: SPACE[4], gap: SPACE[3], backgroundColor: c.inset, borderRadius: RADIUS.lg }}>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[3] }}>
      <View style={{ width: 52, height: 52, borderRadius: RADIUS.full, alignItems: 'center', justifyContent: 'center', backgroundColor: withAlpha(tone, 0.18) }}>
        <Icon name={ok ? 'Check' : 'TriangleAlert'} size={24} color={tone} />
      </View>
      <View style={{ flex: 1, minWidth: 0, gap: SPACE[0.5] }}>
        <Text accessibilityRole="header" style={{ color: c.text, ...theme.type.heading }}>{title}</Text>
        {server ? <Text style={{ color: c.muted, fontSize: theme.base.text.xs }}>
          {m.status_health_sub({ uptime: since(server.started_at), version: server.version })}
        </Text> : null}
      </View>
    </View>
    {ok ? null : <View style={{ gap: SPACE[1] }}>
      {attention.map((a, i) => {
        const color = a.severity === 'bad' ? theme.tokens.status.error : theme.tokens.status.warning;
        const text = attentionText(a);
        return <Pressable key={`${a.code}:${i}`} accessibilityRole="button" accessibilityLabel={text}
          onPress={() => { haptic.selection(); onJump(a.target); }}
          style={({ pressed }) => ({ minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: SPACE[2], paddingHorizontal: SPACE[3],
            borderRadius: RADIUS.sm, borderWidth: 1, borderColor: c.border, backgroundColor: pressed ? c.hover : 'transparent' })}>
          <Icon name="TriangleAlert" size={14} color={color} />
          <Text style={{ flex: 1, color: c.text, fontSize: theme.base.text.sm }}>{text}</Text>
          <Icon name="ChevronRight" size={14} color={c.faint} />
        </Pressable>;
      })}
    </View>}
  </View>;
}

function StatTile({ label, value, sub, values, color, format, a11y }: { label: string; value: string; sub?: string; values: number[]; color: string; format: (v: number, ago: number) => string; a11y: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={{ flex: 1, minWidth: 0, padding: SPACE[3], gap: SPACE[1], backgroundColor: c.inset, borderRadius: RADIUS.lg }}>
    <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{label}</Text>
    <Text numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7} style={{ color: c.text, fontSize: theme.base.text.xl, fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] }}>{value}</Text>
    {sub ? <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{sub}</Text> : null}
    <Sparkline values={values} color={color} format={format} label={a11y} />
    <View style={{ flexDirection: 'row', justifyContent: 'space-between' }}>
      <Text style={{ color: c.faint, fontSize: theme.base.text.xxxs }}>{m.status_axis_hour_ago()}</Text>
      <Text style={{ color: c.faint, fontSize: theme.base.text.xxxs }}>{m.status_axis_now()}</Text>
    </View>
  </View>;
}

export function ServerTiles({ server, onLayout }: { server: StatusServerBlock } & Place) {
  const colors = useChartColors();
  const mem = server.rss_series.map((p) => p.v);
  const cpu = server.cpu_series.map((p) => p.v);
  const memAvg = server.rss_avg_bytes !== undefined ? m.status_server_avg({ value: bytes(server.rss_avg_bytes) }) : undefined;
  const cpuAvg = server.cpu_avg_pct !== undefined ? m.status_server_avg({ value: pct(server.cpu_avg_pct) }) : undefined;
  return <View onLayout={(e) => onLayout?.(e.nativeEvent.layout.y)} style={{ flexDirection: 'row', gap: SPACE[2] }}>
    <StatTile label={m.status_server_memory()} value={bytes(server.rss_bytes)} sub={memAvg} values={mem} color={colors[0]}
      a11y={[m.status_server_memory(), bytes(server.rss_bytes), memAvg].filter(Boolean).join(', ')}
      format={(v, ago) => m.status_point_ago({ value: bytes(v), n: String(ago) })} />
    <StatTile label={m.status_server_cpu()} value={pct(server.cpu_pct)} sub={cpuAvg} values={cpu} color={colors[2]}
      a11y={[m.status_server_cpu(), pct(server.cpu_pct), cpuAvg].filter(Boolean).join(', ')}
      format={(v, ago) => m.status_point_ago({ value: pct(v), n: String(ago) })} />
  </View>;
}

const DAY = 86_400_000;

export function DaysCard({ exits, update, flash, onLayout }: { exits?: StatusExitsBlock; update?: StatusUpdateBlock } & Place) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [width, setWidth] = useState(300);
  const [picked, setPicked] = useState<number | null>(null);
  const items = exits?.items ?? [];
  const start = new Date(); start.setHours(0, 0, 0, 0);
  const from = start.getTime() - 6 * DAY;
  const days = Array.from({ length: 7 }, (_, i) => new Date(from + i * DAY));
  const at = (ts: number) => Math.max(0, Math.min(1, (ts - from) / (7 * DAY))) * width;
  const crashes = items.filter((e) => e.kind === 'crash').length;
  const updates = items.filter((e) => e.kind === 'update').length;
  const hint = !exits ? undefined : [
    crashes === 0 ? m.status_days_none() : crashes === 1 ? m.status_days_crashes_one() : m.status_days_crashes({ count: String(crashes) }),
    ...(updates === 0 ? [] : [updates === 1 ? m.status_days_updates_one() : m.status_days_updates({ count: String(updates) })]),
  ].join(' · ');
  const colorOf = (e: StatusExit) => e.kind === 'crash' ? theme.tokens.status.error : e.kind === 'update' ? c.accent : c.faint;
  const sel = picked !== null ? items[picked] : undefined;
  const upd = update ? updateLines(update) : null;
  const kinds = new Set(items.map((e) => e.kind));
  return <Block title={m.status_days_title()} hint={hint} flash={flash} onLayout={onLayout}>
    {exits ? <>
      <View onLayout={(e) => setWidth(Math.max(1, e.nativeEvent.layout.width))} style={{ height: 52 }}>
        <View style={{ position: 'absolute', left: 0, right: 0, top: 14, height: 2, borderRadius: 1, backgroundColor: c.borderStrong }} />
        {days.map((d, i) => <View key={i} style={{ position: 'absolute', left: i / 7 * width, width: width / 7, top: 26, alignItems: 'center' }}>
          <Text numberOfLines={1} style={{ color: c.faint, fontSize: theme.base.text.xxxs }}>
            {`${d.toLocaleDateString(intlLocale(), { weekday: 'short' }).replace(/\.$/, '')} ${d.getDate()}`}
          </Text>
        </View>)}
        {days.slice(1).map((_, i) => <View key={`t${i}`} style={{ position: 'absolute', left: (i + 1) / 7 * width, top: 10, width: 1, height: 10, backgroundColor: c.border }} />)}
        {items.map((e, i) => {
          const color = colorOf(e);
          const label = `${exitKindText(e.kind)} · ${when(e.ts)}${exitDetail(e) ? ` · ${exitDetail(e)}` : ''}`;
          return <Pressable key={`${e.ts}:${i}`} accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ selected: picked === i }}
            hitSlop={SPACE[4]} onPress={() => { haptic.selection(); setPicked(picked === i ? null : i); }}
            style={{ position: 'absolute', left: at(Date.parse(e.ts)) - 7, top: 8, width: 14, height: 14, alignItems: 'center', justifyContent: 'center' }}>
            <View style={{ width: e.kind === 'update' ? 10 : 12, height: e.kind === 'update' ? 10 : 12, backgroundColor: color,
              borderRadius: e.kind === 'update' ? 2 : RADIUS.full, borderWidth: picked === i ? 2 : 0, borderColor: c.text,
              transform: e.kind === 'update' ? [{ rotate: '45deg' }] : undefined }} />
          </Pressable>;
        })}
      </View>
      <Legend items={[
        { label: m.status_exit_crash(), color: theme.tokens.status.error, shape: 'dot' },
        { label: m.status_exit_update(), color: c.accent, shape: 'diamond' },
        ...(kinds.has('requested') ? [{ label: m.status_exit_requested(), color: c.faint, shape: 'dot' as const }] : []),
      ]} />
      {sel ? <View style={{ gap: SPACE[1], padding: SPACE[3], borderRadius: RADIUS.sm, backgroundColor: c.hover }}>
        <Text style={{ color: c.text, fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold }}>{`${exitKindText(sel.kind)} · ${when(sel.ts)}`}</Text>
        {exitDetail(sel) ? <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{exitDetail(sel)}</Text> : null}
        {sel.error_lines.map((line, i) => <Mono key={i} color={c.muted}>{line}</Mono>)}
      </View> : null}
    </> : null}
    {upd ? <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[2], paddingTop: SPACE[2], borderTopWidth: exits ? 1 : 0, borderTopColor: c.border }}>
      <View style={{ width: 28, height: 28, borderRadius: RADIUS.sm, alignItems: 'center', justifyContent: 'center', backgroundColor: c.accentDim }}>
        <Icon name="ArrowUp" size={15} color={c.accentText} />
      </View>
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={{ color: c.text, fontSize: theme.base.text.xs }}>{upd.title}</Text>
        <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{upd.detail}</Text>
      </View>
    </View> : null}
  </Block>;
}

function machineTag(mc: StatusMachine): string {
  if (mc.this_machine) return m.status_machines_this();
  if (mc.lan) return m.status_machines_lan();
  return mc.app ? platformName(mc.app.platform) : '';
}

function MachineCard({ mc }: { mc: StatusMachine }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const dot = mc.state === 'ok' ? theme.tokens.status.success : mc.state === 'down' ? theme.tokens.status.error : c.faint;
  const state = mc.state === 'down' ? (mc.down_since ? m.status_machines_down_for({ time: since(mc.down_since) }) : m.status_machines_down())
    : mc.state === 'unknown' ? m.status_machines_unknown() : mc.latency_ms === undefined ? m.status_machines_up() : undefined;
  const tag = machineTag(mc);
  return <View style={{ padding: SPACE[3], gap: SPACE[1.5], borderRadius: RADIUS.md, borderWidth: 1, borderColor: c.border }}>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[2] }}>
      <View style={{ width: 8, height: 8, borderRadius: RADIUS.full, backgroundColor: dot }} />
      <Text numberOfLines={1} style={{ flexShrink: 1, color: c.text, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold }}>{mc.label}</Text>
      {tag ? <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{tag}</Text> : null}
      <View style={{ flex: 1 }} />
      {state ? <Text style={{ color: mc.state === 'down' ? theme.tokens.status.error : c.muted, fontSize: theme.base.text.xxs }}>{state}</Text>
        : <><Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{m.status_machines_latency({ ms: String(mc.latency_ms) })}</Text><Signal ms={mc.latency_ms!} /></>}
    </View>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[1.5] }}>
      <Icon name="Server" size={13} color={c.faint} />
      <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{m.status_machines_server()}</Text>
      <Mono>{mc.server_version ?? '—'}</Mono>
      {mc.outdated ? <Text style={{ color: theme.tokens.status.warning, fontSize: theme.base.text.xxxs, paddingHorizontal: SPACE[1.5], borderRadius: RADIUS.full, backgroundColor: withAlpha(theme.tokens.status.warning, 0.16) }}>{m.status_machines_outdated()}</Text> : null}
    </View>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[1.5] }}>
      <Icon name="Monitor" size={13} color={c.faint} />
      {mc.app ? <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>
        {`${m.status_machines_app()} `}<Text style={{ color: c.text }}>{`${bytes(mc.app.rss_bytes)} · ${pct(mc.app.cpu_pct)} · ${mc.app.version}`}</Text>
      </Text> : <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{m.status_machines_no_app()}</Text>}
    </View>
  </View>;
}

export function MachinesCard({ machines, flash, onLayout }: { machines: StatusMachine[] } & Place) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Block title={m.status_machines_title()} hint={m.status_machines_hint()} flash={flash} onLayout={onLayout}>
    <View style={{ gap: SPACE[2] }}>{machines.map((mc) => <MachineCard key={mc.id} mc={mc} />)}</View>
    {machines.every((mc) => mc.this_machine) ? <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{m.status_machines_only_this()}</Text> : null}
  </Block>;
}

export function SessionsCard({ sessions, flash, onLayout }: { sessions: StatusSessionsBlock } & Place) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const colors = useChartColors();
  const [mode, setMode] = useState<'provider' | 'process'>('provider');
  if (sessions.open === 0) {
    return <Block title={m.status_sessions_title()} hint={m.status_sessions_none_hint()} flash={flash} onLayout={onLayout}>
      <View style={{ height: 6, borderRadius: RADIUS.full, backgroundColor: c.hover }} />
      <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{m.status_sessions_none()}</Text>
    </Block>;
  }
  const both = sessions.by_provider.length > 0 && sessions.by_process.length > 0;
  const list: StatusUsage[] = mode === 'process' && sessions.by_process.length ? sessions.by_process : sessions.by_provider.length ? sessions.by_provider : sessions.by_process;
  const nameOf = list === sessions.by_process ? processName : providerLabel;
  const total = sessions.by_provider.reduce((sum, u) => sum + (u.rss_bytes ?? 0), 0);
  const memory = total > 0 ? bytes(total) : '—';
  const hint = sessions.open === 1 ? m.status_sessions_hint_one({ memory }) : m.status_sessions_hint({ count: String(sessions.open), memory });
  const items = list.map((u, i) => ({ label: nameOf(u.key), value: u.rss_bytes ?? 0, color: colors[i % colors.length], detail: u.rss_bytes !== undefined ? bytes(u.rss_bytes) : '—', count: u.count }));
  const dead = sessions.dead;
  return <Block title={m.status_sessions_title()} hint={hint} flash={flash} onLayout={onLayout}
    right={both ? <Segmented label={m.status_sessions_split()} value={mode} onChange={(k) => setMode(k as 'provider' | 'process')}
      options={[{ key: 'provider', label: m.status_sessions_by_provider() }, { key: 'process', label: m.status_sessions_by_process() }]} /> : undefined}>
    <SegBar label={m.status_sessions_title()} items={items} />
    <Legend items={items.map((it) => ({ label: it.label, color: it.color, extra: `${it.count} · ${it.detail}` }))} />
    {dead.length || sessions.asleep || sessions.queued ? <Chips>
      {dead.length ? <Chip tone="bad" text={dead.length === 1 ? m.status_sessions_dead_one({ name: dead[0].name })
        : m.status_sessions_dead({ count: String(dead.length), names: dead.map((d) => d.name).join(', ') })} /> : null}
      {sessions.asleep ? <Chip text={sessions.asleep === 1 ? m.status_sessions_asleep_one() : m.status_sessions_asleep({ count: String(sessions.asleep) })} /> : null}
      {sessions.queued ? <Chip text={sessions.oldest_queued_secs !== undefined
        ? m.status_sessions_queued_for({ count: String(sessions.queued), time: duration(sessions.oldest_queued_secs * 1000) })
        : m.status_sessions_queued({ count: String(sessions.queued) })} /> : null}
    </Chips> : null}
  </Block>;
}

function hourOf(iso: string) {
  return `${String(new Date(iso).getHours()).padStart(2, '0')}h`;
}

export function SlowCard({ slow, flash, onLayout }: { slow: StatusSlowBlock } & Place) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const nSlow = slow.hours.reduce((sum, h) => sum + h.slow, 0);
  const nErr = slow.hours.reduce((sum, h) => sum + h.errors, 0);
  const hours: HourColumn[] = slow.hours.map((h) => ({ hour: hourOf(h.hour_start), slow: h.slow, errors: h.errors }));
  const top = Math.max(1, ...slow.top_routes.map((r) => r.count));
  return <Block title={m.status_slow_title()} hint={m.status_slow_hint({ slow: String(nSlow), errors: String(nErr) })} flash={flash} onLayout={onLayout}>
    <HourColumns hours={hours} label={m.status_slow_chart()}
      describe={(h) => m.status_slow_hour({ hour: h.hour, slow: String(h.slow), errors: String(h.errors) })} />
    <View style={{ flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: SPACE[3] }}>
      <Legend items={[{ label: m.status_slow_legend_slow(), color: theme.tokens.status.warning }, { label: m.status_slow_legend_errors(), color: theme.tokens.status.error }]} />
      {slow.watchdog.length ? <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[1] }}>
        <Icon name="TriangleAlert" size={12} color={c.muted} />
        <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>
          {slow.watchdog.length === 1 ? m.status_slow_watchdog_one() : m.status_slow_watchdog({ count: String(slow.watchdog.length) })}
        </Text>
      </View> : null}
    </View>
    {slow.top_routes.length ? <View style={{ gap: SPACE[1.5] }}>
      {slow.top_routes.map((r) => {
        const failed = r.last_code >= 500;
        const value = failed || r.worst_ms === undefined ? String(r.last_code) : m.status_seconds({ n: num(r.worst_ms / 1000, 1) });
        return <View key={r.route} accessible accessibilityLabel={`${r.route}: ${r.count} · ${value}`} style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[2] }}>
          <View style={{ flex: 1, minWidth: 0 }}><Mono>{r.route}</Mono></View>
          <View style={{ width: 4 + r.count / top * 72, height: 6, borderRadius: RADIUS.full, backgroundColor: failed ? theme.tokens.status.error : theme.tokens.status.warning }} />
          <Text style={{ width: 44, textAlign: 'right', color: c.muted, fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] }}>{value}</Text>
        </View>;
      })}
    </View> : null}
  </Block>;
}

export function HostCard({ host, flash, onLayout }: { host: StatusHostBlock } & Place) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const colors = useChartColors();
  const hint = host.load ? m.status_host_hint({ load: num(host.load[0], 2), cpus: String(host.cpus) }) : m.status_host_hint_cpus({ cpus: String(host.cpus) });
  const meters: { name: string; used: number; total: number }[] = [
    { name: m.status_server_memory(), used: host.mem_total - host.mem_available, total: host.mem_total },
    ...(host.swap_total > 0 ? [{ name: m.status_host_swap(), used: host.swap_used, total: host.swap_total }] : []),
    { name: m.status_host_disk(), used: host.disk_home.used, total: host.disk_home.total },
    ...(host.disk_tmp ? [{ name: m.status_host_tmp(), used: host.disk_tmp.used, total: host.disk_tmp.total }] : []),
  ];
  const hangar = host.dir_hangar.bytes;
  const build = host.dir_build_cache.bytes;
  const free = Math.max(0, host.disk_home.total - host.disk_home.used);
  const other = Math.max(0, host.disk_home.used - (hangar ?? 0) - (build ?? 0));
  const part = (label: string, value: number | undefined, computing: boolean, color: string) =>
    value === undefined ? { label, value: 0, color, detail: computing ? m.status_host_computing() : '—' } : { label, value, color, detail: bytes(value) };
  const disk = [
    part(m.status_host_dir_hangar(), hangar, host.dir_hangar.computing, colors[0]),
    part(m.status_host_dir_build(), build, host.dir_build_cache.computing, colors[1]),
    { label: m.status_host_other(), value: other, color: colors[5], detail: bytes(other) },
    { label: m.status_host_free(), value: free, color: c.hover, detail: bytes(free) },
  ];
  return <Block title={m.status_host_title()} hint={hint} flash={flash} onLayout={onLayout}>
    <View>{meters.map((mt) => {
      const text = `${bytes(mt.used)} / ${bytes(mt.total)}`;
      const ratio = mt.total > 0 ? mt.used / mt.total : 0;
      return <Meter key={mt.name} name={mt.name} used={mt.used} total={mt.total} text={text}
        label={m.status_host_meter({ name: mt.name, pct: num(ratio * 100), used: bytes(mt.used), total: bytes(mt.total) })} />;
    })}</View>
    <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{m.status_host_disk_use()}</Text>
    <Composition label={m.status_host_disk_use()} items={disk} />
    <Legend items={disk.map((d) => ({ label: d.label, color: d.color, extra: d.detail }))} />
  </Block>;
}

function Tile({ icon, color, title, value }: { icon: IconName; color: string; title: string; value: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View accessible accessibilityLabel={`${title}: ${value}`} style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[3], padding: SPACE[3], borderRadius: RADIUS.md, borderWidth: 1, borderColor: c.border }}>
    <View style={{ width: 32, height: 32, borderRadius: RADIUS.sm, alignItems: 'center', justifyContent: 'center', backgroundColor: withAlpha(color, 0.16) }}>
      <Icon name={icon} size={16} color={color} />
    </View>
    <View style={{ flex: 1, minWidth: 0 }}>
      <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{title}</Text>
      <Text style={{ color: c.text, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium }}>{value}</Text>
    </View>
  </View>;
}

export function OutsideCard({ outside, leftovers, flash, onLayout }: { outside?: StatusOutsideBlock; leftovers?: StatusLeftoversBlock } & Place) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const good = theme.tokens.status.success;
  const warn = theme.tokens.status.warning;
  const connect = outside?.connect;
  const tmux = leftovers?.tmux_orphans;
  const cano = leftovers?.cano_orphans;
  return <Block title={m.status_outside_title()} flash={flash} onLayout={onLayout}>
    <View style={{ gap: SPACE[2] }}>
      {connect ? <Tile icon="Globe" title={m.status_outside_connect()}
        color={!connect.enabled ? c.faint : connect.running ? good : warn}
        value={!connect.enabled ? m.status_off() : connect.running ? m.status_on() : m.status_connect_not_running()} /> : null}
      {outside?.funnel !== undefined ? <Tile icon="Network" title={m.status_outside_funnel()} color={outside.funnel ? good : c.faint}
        value={outside.funnel ? m.status_on() : m.status_off()} /> : null}
      {outside?.keyring_locked !== undefined ? <Tile icon="KeyRound" title={m.status_outside_keyring()} color={outside.keyring_locked ? warn : good}
        value={outside.keyring_locked ? m.status_keyring_locked() : m.status_keyring_unlocked()} /> : null}
    </View>
    {tmux !== undefined || cano !== undefined ? <Chips>
      {tmux !== undefined ? tmux.length ? <Chip icon="Recycle" text={m.status_leftover_tmux({ count: String(tmux.length), names: tmux.join(', ') })} />
        : <Chip icon="Check" tone="ok" text={m.status_leftover_tmux_none()} /> : null}
      {cano !== undefined ? cano ? <Chip icon="Recycle" text={cano === 1 ? m.status_leftover_cano_one() : m.status_leftover_cano({ count: String(cano) })} />
        : <Chip icon="Check" tone="ok" text={m.status_leftover_cano_none()} /> : null}
    </Chips> : null}
  </Block>;
}
