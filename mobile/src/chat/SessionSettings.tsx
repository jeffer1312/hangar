import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Text, View, useWindowDimensions } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { parseStatusLine, type ClaudeProviderSource } from '@hangar/core';
import { chatStore } from '../stores/chat';
import { useSessions } from '../stores/sessions';
import { useServers } from '../stores/servers';
import { sessionAccountLabel } from './sessionAccountLabel';
import { SessionAccountSummary } from './SessionAccountSummary';
import { ClaudeProviderSettings, useClaudeProviderEngines } from './ClaudeProviderSettings';
import { AnchoredPanel } from '../ui/AnchoredPanel';
import { Sheet } from '../ui/Sheet';
import { UsagePanel } from './UsagePanel';
import { SessionSettingsContent, SessionSettingsFrame, SessionSettingsLink } from './SessionSettingsFrame';
import { SettingsList } from '../ui/SettingsList';
import { Icon, type IconName } from '../ui/Icon';
import * as m from '../paraglide/messages';
import { useModelControl } from '../features/pills/modelControl';
import { useEffortControl } from '../features/pills/effortControl';
import { SPEED_TIERS, speedHint, speedLabel, useServiceTierControl } from '../features/pills/serviceTierControl';
import { haptic } from '../ui/haptics';
import { usePermissionControl } from '../features/pills/permissionControl';
import { approvalShortLabel, useCodexApproval } from '../features/pills/codexApproval';
import { permissionLabel } from '../features/pills/permissionLabel';
import { reconcileChosen, type Chosen } from '../features/pills/pills';

interface Props {
  serverId: string;
  name: string;
  provider: string | null;
  headless: boolean;
  openRequest?: { which: 'model' | 'effort' | 'mode' | 'account' | 'usage'; n: number } | null;
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
  const [tab, setTab] = useState<'model' | 'session' | 'usage'>('model');
  const [detail, setDetail] = useState<'models' | 'effort' | 'mode' | 'approval' | 'providers' | null>(null);
  const [providerBusy, setProviderBusy] = useState(false);
  const providerInventory = useClaudeProviderEngines(server, open && canSwitchAccount && !!session?.lifecycle_id);
  const source = session?.lifecycle_id && session.jsonl && !session.guest_kind && canSwitchAccount
    ? { lifecycle_id: session.lifecycle_id, jsonl: session.jsonl, headless: !!session.headless, engine: session.engine ?? null,
      engine_account: session.engine_account ?? null, engine_binding: session.engine_binding ?? null } : null;
  const providerCatalog = source && server && providerInventory.result?.inventory?.claude_provider_switch ? providerInventory.result.inventory : null;
  const applyProviderModel = (origin: ClaudeProviderSource) => {
    const current = useSessions.getState().rows.find(row => row.serverId === serverId && row.name === name);
    const connected = useServers.getState().servers.find(row => row.id === serverId);
    if (current?.jsonl !== origin.jsonl || current.lifecycle_id !== origin.lifecycle_id || current.provider !== 'claude'
      || connected?.baseUrl !== server?.baseUrl || connected?.token !== server?.token) return;
    setChosen({}); setDetail(null); setOpen(false);
  };
  const afterDismiss = useRef<(() => void) | null>(null);
  const { height } = useWindowDimensions();
  const close = useCallback(() => { if (!providerBusy) setOpen(false); }, [providerBusy]);
  const navigateAfterClose = (path: string) => {
    afterDismiss.current = () => router.push(path as never);
    close();
  };
  useEffect(() => {
    setChosen((cur) => {
      const next = cur.model && cur.model === session?.model ? { ...cur, model: null } : reconcileChosen(statusModel, cur);
      return next.model === cur.model ? cur : next;
    });
  }, [statusModel, session?.model]);
  const isCodex = provider === 'codex';
  const isClaude = !provider || provider === 'claude';
  const permission = usePermissionControl({ serverId, name, provider: isCodex ? 'codex' : isClaude ? 'claude' : null });
  const approval = useCodexApproval({ serverId, name, headless, enabled: isCodex });
  const choose = useCallback((next: Chosen) => { setChosen(next); setTab('model'); setDetail(null); }, []);
  const model = useModelControl({ serverId, name, provider, chosen, onChosen: choose, close });
  const effort = useEffortControl({ serverId, name, provider, model: model.model, chosen, onChosen: choose, close });
  const fast = useServiceTierControl({ serverId, name, provider, model: model.model, open });
  useEffect(() => {
    if (effort.hidden) setDetail(current => current === 'effort' ? null : current);
  }, [effort.hidden]);
  useEffect(() => {
    if (!openRequest?.n || (openRequest.which === 'effort' && effort.hidden)) return;
    setTab(openRequest.which === 'usage' ? 'usage' : openRequest.which === 'mode' || openRequest.which === 'account' ? 'session' : 'model');
    setDetail(openRequest.which === 'effort' ? 'effort' : null);
    setOpen(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openRequest?.n]);
  useEffect(() => {
    if (!open) return;
    void model.load();
    if (isCodex && headless) void approval.load();
    // Só ao abrir: recarregar a cada statusline nova piscaria a lista aberta.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  useEffect(() => {
    if (open) void effort.load();
    // Os níveis acompanham o modelo, sem reler a cada atualização de tokens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, model.model]);
  const plan = permission.current === 'plan';
  const icon = permission.current
    ? (isCodex && !plan ? (approval.current ? APPROVAL_ICON[approval.current] : undefined) : MODE_ICON[permission.current])
    : undefined;
  const modeText = permission.current
    ? (isCodex ? (plan ? m.chat_mode_plan() : m.chat_mode_normal()) : permissionLabel(permission.current)) : null;
  const pickMode = async (mo: string) => {
    if (mo === permission.current || await permission.select(mo)) setDetail(null);
  };
  const pickApproval = async (mo: string) => {
    if (mo === approval.current || await approval.select(mo)) setDetail(null);
  };
  const accountHeader = <>
      <Title text={m.cota_conta_ativa()} />
      {open && account && session && server && !server.removed && !server.invite && !server.inviteEnded ? <SessionAccountSummary
        key={JSON.stringify([serverId, name, server.baseUrl, server.token, session.conta, session.engine, session.jsonl])}
        server={server} session={session} label={account}
        onPress={canSwitchAccount ? () => navigateAfterClose(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}/account`) : undefined} />
        : account ? <Text style={styles.note}>{account}</Text> : null}
      <Separator />
    </>;
  const modeControls = permission.enabled ? <>
      {permission.options.length ? <SettingsList comfortable searchable={false}
        items={permission.options.map((mo) => ({ id: mo, label: isCodex ? (mo === 'plan' ? m.chat_mode_plan() : m.chat_mode_normal()) : permissionLabel(mo), selected: mo === permission.current, disabled: permission.applying }))}
        onPick={(mo) => void pickMode(mo)} /> : null}
      {permission.probing ? <Skeleton rows={1} /> : permission.canProbe ? <Probe hint={m.native_mode_probe_hint()} onPress={() => void permission.probe()} />
        : permission.noCycle ? <Empty text={m.permissao_sem_ciclo()} /> : null}
      <Notice text={permission.notice} />
    </> : null;
  const approvalControls = permission.enabled && isCodex ? <>
      {approval.loading ? <Skeleton rows={3} /> : approval.error ? <Failure text={approval.error} onRetry={() => void approval.load()} />
        : approval.modes ? (approval.modes.length ? <SettingsList comfortable searchable={false}
          items={approval.modes.map((mo) => ({ id: mo.name, label: approvalShortLabel(mo.name), hint: mo.desc, selected: mo.name === approval.current, disabled: approval.applying }))}
          onPick={(mo) => void pickApproval(mo)} /> : <Empty text={headless ? m.permissao_codex_headless_sem_lista() : m.permissao_codex_sem_lista()} />)
          : <Probe hint={m.native_codex_permission_probe_hint()} onPress={() => void approval.load()} />}
      <Notice text={approval.notice} />
    </> : null;
  const featured = model.items.slice(0, 3);
  const selected = model.items.find(it => it.selected);
  if (selected && !featured.includes(selected)) featured.splice(2, 1, selected);
  return <SessionSettingsControl open={open} onOpen={() => { setTab('model'); setDetail(null); setOpen(true); }} onClose={close}
    model={model} effort={effort} fast={fast} plan={plan} icon={icon} modeText={modeText}
    panel={<Sheet open={open} onDismiss={() => {
      const action = afterDismiss.current;
      afterDismiss.current = null;
      close();
      action?.();
    }} sizes={['auto']} scrollable dismissible={!providerBusy}>
      <SessionSettingsFrame height={Math.min(600, height * 0.78) - 16}
        title={detail === 'models' ? m.native_ctl_all_models() : detail === 'effort' ? m.native_new_chat_reasoning()
          : detail === 'mode' ? m.native_ctl_mode_title() : detail === 'approval' ? m.composer_codex_approval()
          : detail === 'providers' ? m.native_claude_provider_picker() : m.native_session_settings_title()}
        subtitle={`${model.display}${effort.value ? ` · ${effortLabel(effort.value)}` : ''}`}
        tab={detail ? undefined : tab} onTab={next => { setTab(next); setDetail(null); }}
        onBack={detail && (detail !== 'providers' || !providerCatalog) && !providerBusy ? () => setDetail(null) : undefined} onClose={close}
        footer={tab === 'model' && !detail && !effort.hidden ? <SessionSettingsLink label={m.native_new_chat_reasoning()}
          value={effort.value ? effortLabel(effort.value) : '—'} disabled={effort.loading || model.loading} onPress={() => setDetail('effort')} /> : undefined}>
          {tab === 'model' ? <SessionSettingsContent title={!detail ? m.native_ctl_featured_models() : undefined}>
            {detail === 'providers' ? source && server && providerCatalog
              ? <ClaudeProviderSettings key={JSON.stringify([serverId, name])} server={server} name={name} source={source}
                  ready={session?.state === 'idle'} inventory={providerCatalog} onBusy={setProviderBusy}
                  onApplied={applyProviderModel} onCancel={() => setDetail(null)}
                  reloadProviders={providerInventory.reload} />
              : source && !providerInventory.result ? <Skeleton rows={2} />
              : providerInventory.result?.failed ? <Failure text={m.native_claude_provider_read_failed()} onRetry={providerInventory.reload} />
              : <Empty text={m.native_claude_provider_changed()} />
            : detail === 'effort' ? <>
              <Notice text={effort.notice} />
              {effort.loading ? <Skeleton rows={3} /> : effort.error ? <Failure text={effort.error} onRetry={() => void effort.load()} /> : !effort.items.length ? <Empty text={m.modelo_sem_niveis()} />
                : <SettingsList comfortable searchable={false} items={effort.items.map(it => ({ id: it.id ?? it.label, label: effortLabel(it.label), hint: it.hint, selected: it.selected, disabled: effort.applying }))}
                  onPick={id => { const item = effort.items.find(it => (it.id ?? it.label) === id); if (item?.selected) setDetail(null); else void effort.select(id); }} />}
            </> : <>
              <Notice text={model.notice} />
              {model.loading ? <Skeleton rows={3} /> : model.error ? <Failure text={model.error} onRetry={() => void (model.refresh ?? model.load)()} />
                : <SettingsList comfortable searchable={detail === 'models' ? 'always' : false} items={(detail === 'models' ? model.items : featured).map(it => ({ id: modelItemKey(it), label: it.label, hint: it.hint, selected: it.selected, disabled: !!it.unavailable }))}
                  onPick={id => pickModel(model, id, () => setDetail(null))} />}
              {detail ? model.refresh ? <SmallButton label={m.native_voice_models_refresh()} onPress={() => void model.refresh?.()} /> : null
                : <>
                  <SessionSettingsLink label={m.native_ctl_all_models()} onPress={() => setDetail('models')} />
                  {source && open && !providerInventory.result ?
                    <SessionSettingsLink label={m.native_claude_provider_picker()} value={m.comum_carregando()} icon="Cpu" disabled onPress={() => setDetail('providers')} /> : null}
                  {source && providerInventory.result?.inventory?.claude_provider_switch ?
                    <SessionSettingsLink label={m.native_claude_provider_picker()} icon="Cpu" onPress={() => setDetail('providers')} /> : null}
                  {source && providerInventory.result?.failed ?
                    <Failure text={m.native_claude_provider_read_failed()} onRetry={providerInventory.reload} /> : null}
                </>}
            </>}
          </SessionSettingsContent> : tab === 'session' ? <SessionSettingsContent>
            {detail === 'mode' ? modeControls : detail === 'approval' ? approvalControls : <>
              {accountHeader}
              {permission.enabled ? <SessionSettingsLink label={m.native_ctl_mode_title()} value={modeText ?? '—'} onPress={() => setDetail('mode')} /> : null}
              <FastControl fast={fast} comfortable />
              {permission.enabled && isCodex ? <SessionSettingsLink label={m.composer_codex_approval()} value={approval.current ? approvalShortLabel(approval.current) : m.native_ctl_probe()} onPress={() => setDetail('approval')} /> : null}
            </>}
          </SessionSettingsContent>
            : <UsagePanel serverId={serverId} name={name} showTitle={false}
              onOpenLimits={() => navigateAfterClose(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}/codex-limits`)} />}
      </SessionSettingsFrame>
    </Sheet>} />;
}

type ModelView = Pick<ReturnType<typeof useModelControl>, 'display' | 'items' | 'select'> &
  Partial<Pick<ReturnType<typeof useModelControl>, 'failed' | 'loading' | 'error' | 'notice' | 'load' | 'refresh'>>;
const modelItemKey = (item: ModelView['items'][number]) => item.id ?? item.hint ?? item.label;
function pickModel(model: ModelView, id: string, onSelected: () => void) {
  const item = model.items.find(it => modelItemKey(it) === id);
  if (!item) return;
  if (item.selected) onSelected();
  else void model.select(item);
}
type EffortView = Pick<ReturnType<typeof useEffortControl>, 'items' | 'select'> &
  Partial<Pick<ReturnType<typeof useEffortControl>, 'hidden' | 'value' | 'loading' | 'applying' | 'error' | 'notice' | 'load'>>;
export type FastView = Pick<ReturnType<typeof useServiceTierControl>, 'tier' | 'available' | 'choose'> &
  Partial<Pick<ReturnType<typeof useServiceTierControl>, 'hidden' | 'fast' | 'ultrafast' | 'loading' | 'applying' | 'error' | 'notice' | 'load'>>;

// A criação entrega escolhas locais; só o controlador do chat conhece uma sessão viva.
export function SessionSettingsControl({ open, onOpen, onClose, model, effort, fast, plan = false, icon, glyph, modeText, header, children, panel }: {
  open: boolean; onOpen: () => void; onClose: () => void;
  model: ModelView; effort: EffortView; fast: FastView;
  // `glyph`: o chip mostra só ele e a seta; modelo e esforço seguem no valor lido e no painel.
  plan?: boolean; icon?: IconName; glyph?: ReactNode; modeText?: string | null; header?: ReactNode; children?: ReactNode; panel?: ReactNode;
}) {
  const { theme } = useUnistyles();
  const anchor = useRef<View>(null);
  const tint = plan ? theme.tokens.accent.base : theme.tokens.text.secondary;
  const effortText = !plan && !effort.hidden && effort.value ? effort.value : null;
  return <>
    <Pressable ref={anchor} onPress={onOpen} hitSlop={{ top: 7, bottom: 7 }}
      style={({ pressed }) => [styles.chip, { backgroundColor: plan ? theme.tokens.accent.dim : pressed ? theme.tokens.bg.hover : theme.tokens.fillSubtle }]}
      accessibilityRole="button" accessibilityLabel={m.composer_session_settings()} accessibilityHint={m.composer_session_settings_hint()}
      accessibilityValue={{ text: [modeText, model.display, effortText, fast.tier !== 'default' ? speedLabel(fast.tier) : null].filter(Boolean).join(', ') }}>
      {icon ? <Icon name={icon} size={13} color={tint} /> : null}
      {glyph ?? <Text style={styles.chipText} numberOfLines={1}>
        {plan ? <Text style={[styles.chipStrong, { color: tint }]}>{`${m.chat_mode_plan()} · `}</Text> : null}
        <Text style={[plan ? { color: tint } : styles.chipStrong, !plan && { color: model.failed ? theme.tokens.status.error : theme.tokens.text.primary }]}>{model.display}</Text>
        {effortText ? <Text style={{ color: theme.tokens.text.secondary }}>{` ${effortText}`}</Text> : null}
        {fast.tier !== 'default' ? <Text style={{ color: theme.tokens.text.secondary }}>{` · ${speedLabel(fast.tier)}`}</Text> : null}
      </Text>}
      <Icon name="ChevronDown" size={13} color={tint} />
    </Pressable>
    {panel ?? <AnchoredPanel open={open} anchor={anchor.current} onClose={onClose} label={m.composer_session_settings()}
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
            onPick={id => pickModel(model, id, onClose)} />}
        {effort.hidden ? null : <>
          <Separator /><Title text={m.native_new_chat_reasoning()} />
          {effort.loading ? <Skeleton rows={1} /> : effort.error ? <Failure text={effort.error} onRetry={() => void effort.load?.()} />
            : effort.items.length === 0 ? <Empty text={m.modelo_sem_niveis()} /> :
              <View style={[styles.efforts, effort.items.length > 5 && styles.wrap]} accessibilityRole="radiogroup">
                {effort.items.map((it) => <Pressable key={it.label} onPress={() => { if (!it.selected) void effort.select(it.id ?? it.label); }}
                  disabled={effort.applying} hitSlop={{ top: 4, bottom: 4 }}
                  style={({ pressed }) => [styles.effort, effort.items.length <= 5 && styles.share,
                    { backgroundColor: it.selected ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' }, effort.applying && styles.busy]}
                  accessibilityRole="radio" accessibilityLabel={effortLabel(it.label)} accessibilityState={{ selected: !!it.selected, checked: !!it.selected, disabled: effort.applying }}>
                  <Text style={[styles.effortText, it.selected && styles.effortOn]} numberOfLines={1}>{effortLabel(it.label)}</Text>
                </Pressable>)}
              </View>}
          <Notice text={effort.notice} />
        </>}
        <FastControl fast={fast} />
        {children}
      </ScrollView>
    </AnchoredPanel>}
  </>;
}

export function FastControl({ fast, comfortable = false }: { fast: FastView; comfortable?: boolean }) {
  const { theme } = useUnistyles();
  if (fast.hidden) return null;
  const hint = !fast.available ? m.native_ctl_fast_unavailable()
    : [speedHint(fast.tier), fast.ultrafast ? null : m.native_ctl_speed_ultrafast_unavailable()].filter(Boolean).join(' ');
  return <>
    {comfortable ? <Text style={styles.settingText}>{m.native_ctl_speed()}</Text> : <><Separator /><Title text={m.native_ctl_speed()} /></>}
    {fast.loading ? <Skeleton rows={1} /> : fast.error ? <Failure text={fast.error} onRetry={() => void fast.load?.()} /> : <>
      <View style={styles.efforts} accessibilityRole="radiogroup" accessibilityLabel={m.native_ctl_speed()}>
        {SPEED_TIERS.map((tier) => {
          const selected = fast.tier === tier;
          const disabled = !fast.available || !!fast.applying || (tier === 'ultrafast' && !fast.ultrafast)
            || (tier === 'priority' && fast.fast === false);
          return <Pressable key={tier} disabled={disabled} hitSlop={{ top: 4, bottom: 4 }}
            onPress={() => { if (selected) return; haptic.selection(); void fast.choose(tier); }}
            style={({ pressed }) => [styles.effort, styles.share,
              { backgroundColor: selected ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' }, disabled && styles.busy]}
            accessibilityRole="radio" accessibilityLabel={speedLabel(tier)}
            accessibilityState={{ selected, checked: selected, disabled, busy: selected && !!fast.applying }}>
            <Text style={[styles.effortText, selected && styles.effortOn]} numberOfLines={1}>{speedLabel(tier)}</Text>
          </Pressable>;
        })}
      </View>
      <Text style={styles.note}>{hint}</Text>
    </>}
    {fast.applying ? <Skeleton rows={1} /> : null}<Notice text={fast.notice} />
  </>;
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
  settingText: { flex: 1, fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
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
