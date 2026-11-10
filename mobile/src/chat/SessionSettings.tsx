import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Switch, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { parseStatusLine } from '@hangar/core';
import { chatStore } from '../stores/chat';
import { useSessions } from '../stores/sessions';
import { useServers } from '../stores/servers';
import { sessionAccountLabel } from './sessionAccountLabel';
import { SessionAccountSummary } from './SessionAccountSummary';
import { AnchoredPanel } from '../ui/AnchoredPanel';
import { SettingsList } from '../ui/SettingsList';
import { Icon, type IconName } from '../ui/Icon';
import * as m from '../paraglide/messages';
import { useModelControl } from '../features/pills/modelControl';
import { useEffortControl } from '../features/pills/effortControl';
import { useServiceTierControl } from '../features/pills/serviceTierControl';
import { usePermissionControl } from '../features/pills/permissionControl';
import { approvalShortLabel, useCodexApproval } from '../features/pills/codexApproval';
import { permissionLabel } from '../features/pills/permissionLabel';
import { reconcileChosen, type Chosen } from '../features/pills/pills';

interface Props {
  serverId: string;
  name: string;
  provider: string | null;
  headless: boolean;
  openRequest?: { which: 'model' | 'effort' | 'mode' | 'account'; n: number } | null;
}

const MODE_ICON: Record<string, IconName> = {
  manual: 'Hand', auto: 'Sparkles', acceptEdits: 'Pencil', plan: 'Diamond', bypassPermissions: 'Zap', dontAsk: 'ShieldOff',
};
const APPROVAL_ICON: Record<string, IconName> = { 'Ask for approval': 'Hand', 'Approve for me': 'Sparkles', 'Full Access': 'Zap' };
const LEVEL: Record<string, () => string> = {
  low: m.reasoning_level_low, medium: m.reasoning_level_medium, high: m.reasoning_level_high,
  xhigh: m.reasoning_level_xhigh, max: m.reasoning_level_max, ultra: m.reasoning_level_ultra,
};
const effortLabel = (lv: string) => LEVEL[lv.toLowerCase()]?.() ?? lv.charAt(0).toUpperCase() + lv.slice(1);

export function SessionSettingsButton({ serverId, name, provider, headless, openRequest }: Props) {
  const router = useRouter();
  const server = useServers((s) => s.servers.find((row) => row.id === serverId) ?? null);
  const canSwitchAccount = provider === 'claude' && !!server && !server.removed && !server.invite && !server.inviteEnded;
  const session = useSessions((s) => s.rows.find((row) => row.serverId === serverId && row.name === name) ?? null);
  const account = session ? sessionAccountLabel(session) : null;
  const chat = chatStore(serverId, name);
  const statusLine = chat.use((s) => s.statusLine);
  const statusModel = useMemo(() => parseStatusLine(statusLine), [statusLine]);
  const [chosen, setChosen] = useState<Chosen>({});
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  useEffect(() => {
    setChosen((cur) => {
      const next = reconcileChosen(statusModel, cur);
      return next.model === cur.model ? cur : next;
    });
  }, [statusModel]);
  const isCodex = provider === 'codex';
  const isClaude = !provider || provider === 'claude';
  const permission = usePermissionControl({ serverId, name, provider: isCodex ? 'codex' : isClaude ? 'claude' : null });
  const approval = useCodexApproval({ serverId, name, headless, enabled: isCodex });
  const model = useModelControl({ serverId, name, provider, chosen, onChosen: setChosen, close });
  const effort = useEffortControl({ serverId, name, provider, chosen, onChosen: setChosen, close });
  const fast = useServiceTierControl({ serverId, name, provider, open });
  useEffect(() => {
    if (!openRequest?.n || (openRequest.which === 'effort' && effort.hidden)) return;
    setOpen(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openRequest?.n]);
  useEffect(() => {
    if (!open) return;
    void model.load();
    void effort.load();
    if (isCodex && headless) void approval.load();
    // Só ao abrir: recarregar a cada statusline nova piscaria a lista aberta.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  const plan = permission.current === 'plan';
  const icon = permission.current
    ? (isCodex && !plan ? (approval.current ? APPROVAL_ICON[approval.current] : undefined) : MODE_ICON[permission.current])
    : undefined;
  const modeText = permission.current
    ? (isCodex ? (plan ? m.chat_mode_plan() : m.chat_mode_normal()) : permissionLabel(permission.current)) : null;
  const pickMode = async (mo: string) => {
    if (mo === permission.current || await permission.select(mo)) close();
  };
  const pickApproval = async (mo: string) => {
    if (mo === approval.current || await approval.select(mo)) close();
  };
  return <SessionSettingsControl open={open} onOpen={() => setOpen(true)} onClose={close}
    model={model} effort={effort} fast={fast} plan={plan} icon={icon} modeText={modeText}
    header={<>
      <Title text={m.cota_conta_ativa()} />
      {open && account && session && server && !server.removed && !server.invite && !server.inviteEnded ? <SessionAccountSummary
        key={JSON.stringify([serverId, name, server.baseUrl, server.token, session.conta, session.engine, session.jsonl])}
        server={server} session={session} label={account}
        onPress={canSwitchAccount ? () => { close(); router.push(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}/account` as never); } : undefined} />
        : account ? <Text style={styles.note}>{account}</Text> : null}
      <Separator />
    </>}>
    {permission.enabled ? <>
      <Separator /><Title text={m.native_ctl_mode_title()} />
      {permission.options.length ? <ChoiceChips busy={permission.applying}
        items={permission.options.map((mo) => ({ key: mo, label: isCodex ? (mo === 'plan' ? m.chat_mode_plan() : m.chat_mode_normal()) : permissionLabel(mo), selected: mo === permission.current }))}
        onPick={(mo) => void pickMode(mo)} /> : null}
      {permission.probing ? <Skeleton rows={1} /> : permission.canProbe ? <Probe hint={m.native_mode_probe_hint()} onPress={() => void permission.probe()} />
        : permission.noCycle ? <Empty text={m.permissao_sem_ciclo()} /> : null}
      <Notice text={permission.notice} />
    </> : null}
    {permission.enabled && isCodex ? <>
      <Separator /><Title text={m.composer_codex_approval()} />
      {approval.loading ? <Skeleton rows={3} /> : approval.error ? <Failure text={approval.error} onRetry={() => void approval.load()} />
        : approval.modes ? (approval.modes.length ? <ChoiceChips busy={approval.applying}
          items={approval.modes.map((mo) => ({ key: mo.name, label: approvalShortLabel(mo.name), aria: mo.name, selected: mo.name === approval.current }))}
          onPick={(mo) => void pickApproval(mo)} /> : <Empty text={headless ? m.permissao_codex_headless_sem_lista() : m.permissao_codex_sem_lista()} />)
          : <Probe hint={m.native_codex_permission_probe_hint()} onPress={() => void approval.load()} />}
      <Notice text={approval.notice} />
    </> : null}
  </SessionSettingsControl>;
}

type ModelView = Pick<ReturnType<typeof useModelControl>, 'display' | 'items' | 'select'> &
  Partial<Pick<ReturnType<typeof useModelControl>, 'failed' | 'loading' | 'error' | 'notice' | 'load' | 'refresh'>>;
const modelItemKey = (item: ModelView['items'][number]) => item.id ?? item.hint ?? item.label;
type EffortView = Pick<ReturnType<typeof useEffortControl>, 'items' | 'select'> &
  Partial<Pick<ReturnType<typeof useEffortControl>, 'hidden' | 'value' | 'loading' | 'applying' | 'error' | 'notice' | 'load'>>;
type FastView = Pick<ReturnType<typeof useServiceTierControl>, 'on' | 'available' | 'toggle'> &
  Partial<Pick<ReturnType<typeof useServiceTierControl>, 'hidden' | 'loading' | 'applying' | 'error' | 'notice' | 'load'>>;

// A criação entrega escolhas locais; só o controlador do chat conhece uma sessão viva.
export function SessionSettingsControl({ open, onOpen, onClose, model, effort, fast, plan = false, icon, modeText, header, children }: {
  open: boolean; onOpen: () => void; onClose: () => void;
  model: ModelView; effort: EffortView; fast: FastView;
  plan?: boolean; icon?: IconName; modeText?: string | null; header?: ReactNode; children?: ReactNode;
}) {
  const { theme } = useUnistyles();
  const anchor = useRef<View>(null);
  const tint = plan ? theme.tokens.accent.base : theme.tokens.text.secondary;
  const effortText = !plan && !effort.hidden && effort.value ? effort.value : null;
  return <>
    <Pressable ref={anchor} onPress={onOpen} hitSlop={{ top: 7, bottom: 7 }}
      style={({ pressed }) => [styles.chip, { backgroundColor: plan ? theme.tokens.accent.dim : pressed ? theme.tokens.bg.hover : theme.tokens.fillSubtle }]}
      accessibilityRole="button" accessibilityLabel={m.composer_session_settings()} accessibilityHint={m.composer_session_settings_hint()}
      accessibilityValue={{ text: [modeText, model.display, effortText, fast.on ? m.native_ctl_fast() : null].filter(Boolean).join(', ') }}>
      {icon ? <Icon name={icon} size={13} color={tint} /> : null}
      <Text style={styles.chipText} numberOfLines={1}>
        {plan ? <Text style={[styles.chipStrong, { color: tint }]}>{`${m.chat_mode_plan()} · `}</Text> : null}
        <Text style={[plan ? { color: tint } : styles.chipStrong, !plan && { color: model.failed ? theme.tokens.status.error : theme.tokens.text.primary }]}>{model.display}</Text>
        {effortText ? <Text style={{ color: theme.tokens.text.secondary }}>{` ${effortText}`}</Text> : null}
        {fast.on ? <Text style={{ color: theme.tokens.text.secondary }}>{` · ${m.native_ctl_fast()}`}</Text> : null}
      </Text>
      <Icon name="ChevronDown" size={13} color={tint} />
    </Pressable>
    <AnchoredPanel open={open} anchor={anchor.current} onClose={onClose} label={m.composer_session_settings()}
      closeLabel={m.native_close()} radius={12} padding={4} maxHeightRatio={1}>
      <ScrollView style={styles.panel} contentContainerStyle={styles.panelContent} keyboardShouldPersistTaps="handled" nestedScrollEnabled>
        {header}
        <Title text={m.native_ctl_model_title()} /><Notice text={model.notice} />
        {model.refresh ? <Pressable accessibilityRole="button" disabled={model.loading}
          onPress={() => void model.refresh?.()} style={{ minHeight: 44, justifyContent: 'center' }}>
          <Text style={[styles.note, { color: theme.tokens.accent.base }]}>{m.native_voice_models_refresh()}</Text>
        </Pressable> : null}
        {model.loading ? <Skeleton rows={5} /> : model.error ? <Failure text={model.error} onRetry={() => void (model.refresh ?? model.load)?.()} /> :
          <SettingsList items={model.items.map((it) => ({ id: modelItemKey(it), label: it.label, hint: it.hint, selected: it.selected, disabled: !!it.unavailable }))}
            onPick={(id) => { const it = model.items.find((item) => modelItemKey(item) === id); if (!it) return; if (it.selected) onClose(); else void model.select(it); }} />}
        {effort.hidden ? null : <>
          <Separator /><Title text={m.native_new_chat_reasoning()} />
          {effort.loading ? <Skeleton rows={1} /> : effort.error ? <Failure text={effort.error} onRetry={() => void effort.load?.()} />
            : effort.items.length === 0 ? <Empty text={m.modelo_sem_niveis()} /> :
              <View style={[styles.efforts, effort.items.length > 5 && styles.wrap]} accessibilityRole="radiogroup">
                {effort.items.map((it) => <Pressable key={it.label} onPress={() => { if (!it.selected) void effort.select(it.id ?? it.label); }}
                  disabled={effort.applying} hitSlop={{ top: 4, bottom: 4 }}
                  style={({ pressed }) => [styles.effort, effort.items.length <= 5 && styles.share,
                    { backgroundColor: it.selected ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' }, effort.applying && styles.busy]}
                  accessibilityRole="radio" accessibilityLabel={effortLabel(it.label)} accessibilityState={{ selected: !!it.selected, disabled: effort.applying }}>
                  <Text style={[styles.effortText, it.selected && styles.effortOn]} numberOfLines={1}>{effortLabel(it.label)}</Text>
                </Pressable>)}
              </View>}
          <Notice text={effort.notice} />
        </>}
        {fast.hidden ? null : <>
          <Separator /><Title text={m.native_ctl_fast()} />
          {fast.loading ? <Skeleton rows={1} /> : fast.error ? <Failure text={fast.error} onRetry={() => void fast.load?.()} /> :
            <View style={styles.fastRow}>
              <Text style={[styles.note, styles.grow]}>{fast.available ? m.native_ctl_fast_hint() : m.native_ctl_fast_unavailable()}</Text>
              <Text style={styles.effortText}>{fast.on ? m.native_ctl_fast_on() : m.native_ctl_fast_off()}</Text>
              <Switch value={fast.on} onValueChange={() => void fast.toggle()} disabled={!fast.available || fast.applying}
                trackColor={{ true: theme.tokens.accent.base }} accessibilityLabel={m.native_ctl_fast()} accessibilityHint={m.native_ctl_fast_hint()}
                accessibilityValue={{ text: fast.on ? m.native_ctl_fast_on() : m.native_ctl_fast_off() }}
                accessibilityState={{ checked: fast.on, disabled: !fast.available || fast.applying, busy: fast.applying }} />
            </View>}
          {fast.applying ? <Skeleton rows={1} /> : null}<Notice text={fast.notice} />
        </>}
        {children}
      </ScrollView>
    </AnchoredPanel>
  </>;
}

function ChoiceChips({ items, onPick, busy }: {
  items: { key: string; label: string; selected: boolean; aria?: string }[]; onPick: (key: string) => void; busy?: boolean;
}) {
  const { theme } = useUnistyles();
  return <View style={[styles.efforts, styles.wrap]} accessibilityRole="radiogroup">
    {items.map((it) => <Pressable key={it.key} onPress={() => { if (!it.selected) onPick(it.key); }} disabled={busy} hitSlop={{ top: 4, bottom: 4 }}
      style={({ pressed }) => [styles.effort, { backgroundColor: it.selected ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' }, busy && styles.busy]}
      accessibilityRole="radio" accessibilityLabel={it.aria ?? it.label} accessibilityState={{ selected: it.selected, disabled: !!busy }}>
      <Text style={[styles.effortText, it.selected && styles.effortOn]} numberOfLines={1}>{it.label}</Text>
    </Pressable>)}
  </View>;
}
function Title({ text }: { text: string }) { return <Text style={styles.title} numberOfLines={1} accessibilityRole="header">{text}</Text>; }
function Separator() { return <View style={styles.separator} />; }
function Skeleton({ rows }: { rows: number }) {
  return <View style={styles.skeleton} accessibilityLabel={m.native_ctl_loading()} accessibilityRole="progressbar">
    {Array.from({ length: rows }, (_, i) => <View key={i} style={styles.bone} />)}
  </View>;
}
function Empty({ text }: { text: string }) { return <Text style={styles.empty}>{text}</Text>; }
function Notice({ text }: { text?: string | null }) { return text ? <Text style={[styles.note, styles.danger]} accessibilityRole="alert">{text}</Text> : null; }
function Failure({ text, onRetry }: { text: string; onRetry: () => void }) {
  return <View style={styles.failure}><Text style={[styles.note, styles.danger]}>{m.native_ctl_failed({ reason: text })}</Text>
    <SmallButton label={m.native_ctl_retry()} onPress={onRetry} /></View>;
}
function Probe({ hint, onPress }: { hint: string; onPress: () => void }) {
  return <View style={styles.probe}><Text style={[styles.note, styles.grow]}>{hint}</Text><SmallButton label={m.native_ctl_probe()} onPress={onPress} /></View>;
}
function SmallButton({ label, onPress }: { label: string; onPress: () => void }) {
  const { theme } = useUnistyles();
  return <Pressable onPress={onPress} hitSlop={{ top: 6, bottom: 6 }} accessibilityRole="button"
    style={({ pressed }) => [styles.small, pressed && { backgroundColor: theme.tokens.fillSubtle }]}><Text style={styles.smallText}>{label}</Text></Pressable>;
}
const styles = StyleSheet.create((theme) => ({
  chip: { flexShrink: 1, minWidth: 0, height: 30, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[2] },
  chipText: { flexShrink: 1, fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  chipStrong: { fontWeight: theme.base.weight.semibold },
  panel: { flexShrink: 1 },
  panelContent: { gap: theme.base.space[0.5] },
  title: { paddingHorizontal: theme.base.space[2], paddingTop: theme.base.space[1.5], paddingBottom: theme.base.space[1], fontSize: theme.base.text.xxxs, fontWeight: theme.base.weight.medium, letterSpacing: 0.4, textTransform: 'uppercase', color: theme.tokens.text.muted },
  separator: { height: 1, marginHorizontal: -4, marginVertical: theme.base.space[0.5], backgroundColor: theme.tokens.border.default },
  efforts: { flexDirection: 'row', gap: theme.base.space[0.5], paddingHorizontal: theme.base.space[1], paddingBottom: theme.base.space[0.5] },
  wrap: { flexWrap: 'wrap' },
  effort: { minHeight: 30, justifyContent: 'center', alignItems: 'center', paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm },
  share: { flex: 1, minWidth: 0, paddingHorizontal: theme.base.space[0.5] },
  effortText: { fontSize: theme.base.text.xxs, color: theme.tokens.text.secondary },
  effortOn: { color: theme.tokens.text.primary, fontWeight: theme.base.weight.semibold },
  busy: { opacity: 0.6 },
  fastRow: { flexDirection: 'row', alignItems: 'center', paddingRight: theme.base.space[2] },
  skeleton: { gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1] },
  bone: { height: 22, borderRadius: theme.base.radius.sm, backgroundColor: theme.tokens.fillSubtle },
  empty: { paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[4], fontSize: theme.base.text.xxs, textAlign: 'center', color: theme.tokens.text.muted },
  note: { paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  danger: { color: theme.tokens.status.error },
  failure: { alignItems: 'flex-start', gap: theme.base.space[1.5], paddingVertical: theme.base.space[1] },
  probe: { flexDirection: 'row', alignItems: 'center', paddingVertical: theme.base.space[0.5] },
  grow: { flex: 1 },
  small: { minHeight: 28, justifyContent: 'center', marginHorizontal: theme.base.space[2], paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, borderWidth: 1, borderColor: theme.tokens.border.default },
  smallText: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.medium, color: theme.tokens.accent.base },
}));
