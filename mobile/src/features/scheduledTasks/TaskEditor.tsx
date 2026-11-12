import { useEffect, useRef, useState } from 'react';
import { Alert, Pressable, Text, View } from 'react-native';
import { Stack, useNavigation } from 'expo-router';
import { usePreventRemove } from 'expo-router/react-navigation';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { StyleSheet } from 'react-native-unistyles';
import { fetchSessionsForServer, getCodexAccountsForServer, getProvidersForServer, listClaudeConfigsForServer,
  getSessionDefaultsForServer, modelOptionsForServer, providerName, SESSION_PROVIDERS, taskDateParts, taskDateToInstant, validateScheduledTask,
  type ConfigDirInfo, type CodexAccount, type ModelOption, type Provider, type ScheduledTask, type ScheduledTaskInput,
  type Server, type SessionInfo, type TaskLaunch, type TaskTarget, type TaskValidation } from '@hangar/core';
import { Pagina } from '../config/Pagina';
import { Segmented } from '../config/Segmented';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { SettingRow } from '../create/SettingRow';
import { CwdPicker } from '../create/CwdPicker';
import { HeaderBack } from '../../ui/glassHeader';
import { useSettingsColors } from '../config/colors';
import { taskWeekdays } from './TaskRow';
import { scheduledError } from './errors';
import * as m from '../../paraglide/messages';

const validations: Record<TaskValidation, () => string> = { name: m.scheduled_invalid_name, prompt: m.scheduled_invalid_prompt,
  days: m.scheduled_invalid_days, time: m.scheduled_invalid_time, interval: m.scheduled_invalid_interval,
  date: m.scheduled_invalid_date, folder: m.scheduled_invalid_folder, session: m.scheduled_select_session };

type Accounts = Array<{ id: string; label: string; disabled?: boolean }>;
export function TaskEditor({ server, timezone, task, onSave, onClose, onReload, clockOffset = 0 }: {
  server: Server; timezone: string; task?: ScheduledTask; onSave: (input: ScheduledTaskInput) => Promise<void>;
  onClose: () => void; onReload: () => Promise<void>; clockOffset?: number;
}) {
  const c = useSettingsColors();
  const navigation = useNavigation();
  const [name, setName] = useState(task?.name ?? '');
  const [prompt, setPrompt] = useState(task?.prompt ?? '');
  const [enabled, setEnabled] = useState(task?.enabled ?? true);
  const [kind, setKind] = useState(task?.schedule.kind ?? 'weekly');
  const [days, setDays] = useState(task?.schedule.kind === 'weekly' ? task.schedule.weekdays : [1, 2, 3, 4, 5]);
  const [once] = useState(() => taskDateParts(task?.schedule.kind === 'once' ? task.schedule.at : new Date(Date.now() + clockOffset + 3600000).toISOString(), timezone));
  const [date, setDate] = useState(once.date);
  const [time, setTime] = useState(task?.schedule.kind === 'weekly' ? task.schedule.time : once.time);
  const [interval, setInterval] = useState(String(task?.schedule.kind === 'interval' ? task.schedule.minutes : 60));
  const [unit, setUnit] = useState<'minutes' | 'hours'>('minutes');
  const [target, setTarget] = useState<TaskTarget>(task?.target ?? { kind: 'new', launch: {}, worktree: false });
  const [launch, setLaunch] = useState<TaskLaunch>(task?.target.kind === 'new' ? task.target.launch : {});
  const [worktree, setWorktree] = useState(task?.target.kind === 'new' && task.target.worktree);
  const [base, setBase] = useState(task?.target.kind === 'new' ? task.target.base ?? '' : '');
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  const [error, setError] = useState('');
  const [conflict, setConflict] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [catalogLoading, setCatalogLoading] = useState(true);
  const [catalogError, setCatalogError] = useState('');
  const [models, setModels] = useState<ModelOption[]>([]);
  const [accounts, setAccounts] = useState<Accounts>([]);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [probe, setProbe] = useState<Record<string, { disponivel: boolean; motivo: string | null }>>({});
  const [defaultModel, setDefaultModel] = useState('');
  const [retry, setRetry] = useState(0);
  const provider = launch.provider ?? 'claude';
  const change = <T,>(set: (value: T) => void) => (value: T) => { set(value); setDirty(true); setError(''); };
  const patchLaunch = (patch: Partial<TaskLaunch>) => { setLaunch((old) => ({ ...old, ...patch })); setDirty(true); setError(''); };
  const discard = (done: () => void) => Alert.alert(m.scheduled_discard_title(), m.scheduled_discard_body(), [
    { text: m.scheduled_keep_editing(), style: 'cancel' }, { text: m.scheduled_discard(), style: 'destructive', onPress: done },
  ]);
  const close = () => { if (lock.current) return; if (dirty) discard(onClose); else onClose(); };
  usePreventRemove(dirty || busy, ({ data }) => { if (!lock.current) discard(() => navigation.dispatch(data.action)); });

  useEffect(() => {
    let alive = true;
    setCatalogLoading(true); setCatalogError('');
    const load = async () => {
      if (target.kind === 'session') {
        const list = await fetchSessionsForServer(server);
        if (alive) setSessions(list.filter((s) => s.state !== 'dead'));
      } else {
        const providers = await getProvidersForServer(server);
        if (!alive) return;
        setProbe(providers);
        if (!launch.provider) {
          const preferred = SESSION_PROVIDERS.find((p) => providers[p]?.default && providers[p]?.disponivel) ?? 'claude';
          setLaunch((old) => ({ ...old, provider: preferred }));
          return;
        }
        const [defaults, modelList, accountList] = await Promise.all([
          getSessionDefaultsForServer(server, provider, launch.engine), modelOptionsForServer(server, provider, launch.engine, launch.config_dir, launch.codex_account),
          provider === 'claude' ? listClaudeConfigsForServer(server) : provider === 'codex' ? getCodexAccountsForServer(server) : Promise.resolve([]),
        ]);
        if (!alive) return;
        setDefaultModel(defaults?.model ?? modelList.default ?? ''); setModels(modelList.models);
        setAccounts(provider === 'claude' ? (accountList as ConfigDirInfo[]).map((a) => ({ id: a.path, label: a.label }))
          : (accountList as CodexAccount[]).map((a) => ({ id: a.id, label: a.name, disabled: a.auth.status !== 'connected' })));
      }
    };
    void load().catch((e) => { if (alive) setCatalogError(scheduledError(e)); }).finally(() => { if (alive) setCatalogLoading(false); });
    return () => { alive = false; };
  }, [server, target.kind, provider, launch.provider, launch.engine, launch.config_dir, launch.codex_account, retry]);

  const save = async () => {
    if (lock.current || catalogLoading || catalogError || (target.kind === 'new' && !launch.provider)) return;
    const schedule = kind === 'weekly' ? { kind, time, weekdays: days } as const
      : kind === 'interval' ? { kind, minutes: Number(interval) * (unit === 'hours' ? 60 : 1) } as const
      : { kind: 'once' as const, at: task?.schedule.kind === 'once' && date === once.date && time === once.time
        ? task.schedule.at : taskDateToInstant(date, time, timezone) ?? '' };
    const input: ScheduledTaskInput = { name: name.trim(), prompt: prompt.trim(), enabled, schedule,
      target: target.kind === 'new' ? { kind: 'new', launch, worktree, ...(base.trim() && worktree ? { base: base.trim() } : {}) } : target };
    const invalid = validateScheduledTask(input, Date.now() + clockOffset, task);
    if (invalid) { setError(validations[invalid]()); return; }
    lock.current = true; setBusy(true); setError('');
    try { await onSave(input); }
    catch (e) { setError(scheduledError(e)); setConflict((e as { status?: number; code?: string }).status === 409 && !!(e as { code?: string }).code?.includes('revision')); }
    finally { lock.current = false; setBusy(false); }
  };
  const reload = () => discard(() => {
    if (lock.current) return;
    lock.current = true; setBusy(true);
    void onReload().catch((e) => setError(scheduledError(e))).finally(() => { lock.current = false; setBusy(false); });
  });
  const accountValue = provider === 'claude' ? launch.config_dir ?? '' : launch.codex_account ?? '';
  const modelValue = (model: ModelOption) => model.provider ? `${model.provider}/${model.id}` : model.id;
  const choices = taskWeekdays();
  return <KeyboardAvoidingView behavior="padding" automaticOffset style={{ flex: 1 }}><Pagina>
    <Stack.Screen options={{ title: task ? m.scheduled_edit() : m.scheduled_new(), headerLeft: () => <HeaderBack onPress={close} /> }} />
    <MachineHelp>{m.scheduled_runs_on({ server: server.label })}</MachineHelp>
    <MachineHelp>{m.scheduled_timezone({ timezone })}</MachineHelp>
    <MachineGroup>
      <MachineField label={m.scheduled_name()} value={name} onChangeText={change(setName)} placeholder={m.scheduled_name_hint()} stacked maxLength={100} editable={!busy} returnKeyType="next" />
      <MachineField label={m.scheduled_prompt()} value={prompt} onChangeText={change(setPrompt)} placeholder={m.scheduled_prompt_hint()} stacked multiline editable={!busy} />
    </MachineGroup>
    <MachineHelp>{m.scheduled_when()}</MachineHelp>
    <Segmented label={m.scheduled_when()} value={kind} disabled={busy} onChange={change(setKind)} minHeight={44} options={[
      { v: 'once', label: m.scheduled_once() }, { v: 'weekly', label: m.scheduled_weekly() }, { v: 'interval', label: m.scheduled_interval() },
    ]} />
    {kind === 'weekly' ? <View style={styles.days} accessibilityRole="radiogroup" accessibilityLabel={m.scheduled_weekdays()}>
      {[1, 2, 3, 4, 5, 6, 0].map((day) => <Pressable key={day} accessibilityRole="checkbox" accessibilityLabel={choices[day]} accessibilityState={{ checked: days.includes(day), disabled: busy }}
        disabled={busy} onPress={() => change(setDays)(days.includes(day) ? days.filter((d) => d !== day) : [...days, day])}
        style={[styles.day, { borderColor: days.includes(day) ? c.accentText : c.borderStrong, backgroundColor: days.includes(day) ? c.accentDim : 'transparent' }]}>
        <Text style={[styles.dayText, { color: days.includes(day) ? c.accentText : c.muted }]}>{choices[day]}</Text>
      </Pressable>)}
    </View> : null}
    <MachineGroup>
      {kind === 'once' ? <MachineField label={m.scheduled_date()} value={date} onChangeText={(value) => { const digits = value.replace(/\D/g, '').slice(0, 8); change(setDate)([digits.slice(0, 2), digits.slice(2, 4), digits.slice(4)].filter(Boolean).join('/')); }} placeholder={m.scheduled_date_hint()} keyboardType="number-pad" maxLength={10} editable={!busy} /> : null}
      {kind !== 'interval' ? <MachineField label={m.scheduled_time()} value={time} onChangeText={(value) => { const digits = value.replace(/\D/g, '').slice(0, 4); change(setTime)([digits.slice(0, 2), digits.slice(2)].filter(Boolean).join(':')); }} placeholder={m.scheduled_time_hint()} keyboardType="number-pad" maxLength={5} editable={!busy} /> : <>
        <MachineField label={m.scheduled_every()} value={interval} onChangeText={change(setInterval)} keyboardType="number-pad" editable={!busy} />
        <SettingRow icon="Clock" label={m.scheduled_interval()} value={unit === 'minutes' ? m.scheduled_minutes() : m.scheduled_hours()} disabled={busy}
          actions={[{ id: 'minutes', title: m.scheduled_minutes() }, { id: 'hours', title: m.scheduled_hours() }]} onAction={(v) => change(setUnit)(v as 'minutes' | 'hours')} />
      </>}
    </MachineGroup>
    <MachineGroup><SettingRow icon="MessageCircle" label={m.scheduled_where()} value={target.kind === 'new' ? m.scheduled_target_new() : m.scheduled_target_session()} disabled={busy}
      actions={[{ id: 'new', title: m.scheduled_target_new() }, { id: 'session', title: m.scheduled_target_session() }]}
      onAction={(v) => change(setTarget)(v === 'new' ? { kind: 'new', launch, worktree } : { kind: 'session', name: '' })} /></MachineGroup>
    {target.kind === 'session' ? <>
      <MachineGroup><SettingRow icon="MessageCircle" label={m.scheduled_session()} value={target.name || m.scheduled_select_session()} disabled={busy || catalogLoading || !!catalogError}
        actions={sessions.map((s) => ({ id: s.name, title: s.name }))} onAction={(name) => change(setTarget)({ kind: 'session', name })} /></MachineGroup>
      <MachineHelp>{m.scheduled_session_queue_hint()}</MachineHelp>
      {!catalogLoading && !catalogError && !sessions.length ? <MachineHelp>{m.scheduled_no_sessions()}</MachineHelp> : null}
    </> : <>
      <MachineGroup>
        <SettingRow icon="Folder" label={m.scheduled_folder()} value={launch.cwd || m.scheduled_choose_folder()} disabled={busy} onPress={() => setFolderOpen(!folderOpen)} />
        {folderOpen ? <CwdPicker server={server} selected={launch.cwd} autoSelect={false} compact onPick={(cwd) => patchLaunch({ cwd })} onDone={() => setFolderOpen(false)} /> : null}
        <SettingRow icon="Activity" label={m.scheduled_agent()} value={launch.provider ? providerName(provider) : m.scheduled_loading()} disabled={busy || catalogLoading}
          actions={SESSION_PROVIDERS.map((p) => ({ id: p, title: providerName(p), attributes: { disabled: probe[p]?.disponivel === false } }))}
          onAction={(p) => { setLaunch({ cwd: launch.cwd, provider: p as Provider }); setDirty(true); }} />
        {provider === 'claude' || provider === 'codex' ? <SettingRow icon="User" label={m.scheduled_account()} value={accounts.find((a) => a.id === accountValue)?.label ?? (accountValue || m.scheduled_account_default())}
          disabled={busy || catalogLoading || !!catalogError} actions={[{ id: '', title: m.scheduled_account_default() }, ...accounts.map((a) => ({ id: a.id, title: a.label, attributes: { disabled: a.disabled } }))]}
          onAction={(value) => patchLaunch(provider === 'claude' ? { config_dir: value || null, model: null } : { codex_account: value || null, model: null })} /> : null}
        <SettingRow icon="Cpu" label={m.scheduled_model()} value={models.find((model) => modelValue(model) === launch.model)?.name ?? launch.model ?? (defaultModel ? `${m.scheduled_model_default()} · ${defaultModel}` : m.scheduled_model_default())}
          disabled={busy || catalogLoading || !!catalogError} actions={[{ id: '', title: m.scheduled_model_default() }, ...models.filter((md) => md.id !== 'default').map((model) => ({ id: modelValue(model), title: model.name ?? model.id }))]}
          onAction={(model) => patchLaunch({ model: model || null })} />
        <MachineRow title={m.scheduled_worktree()} checked={worktree} disabled={busy} onPress={() => change(setWorktree)(!worktree)} description={m.scheduled_worktree_hint()} />
        {worktree ? <MachineField label={m.scheduled_base()} value={base} onChangeText={change(setBase)} editable={!busy} /> : null}
      </MachineGroup>
    </>}
    {catalogLoading ? <MachineHelp>{target.kind === 'session' ? m.scheduled_loading_sessions() : m.scheduled_loading()}</MachineHelp> : null}
    {catalogError ? <><Notice error text={catalogError} /><MachineGroup><MachineRow title={m.scheduled_retry()} action disabled={busy} onPress={() => setRetry((n) => n + 1)} /></MachineGroup></> : null}
    <MachineGroup><MachineRow title={m.scheduled_enabled_on_save()} checked={enabled} disabled={busy} onPress={() => change(setEnabled)(!enabled)} /></MachineGroup>
    {error ? <Notice text={error} error /> : null}
    {conflict ? <MachineGroup><MachineRow title={m.scheduled_reload_draft()} disabled={busy} onPress={reload} /></MachineGroup> : null}
    <MachineGroup>
      <MachineRow title={busy ? m.scheduled_saving() : task ? m.scheduled_save() : m.scheduled_create()} action disabled={busy || catalogLoading || !!catalogError || conflict || (target.kind === 'new' && !launch.provider)} onPress={() => void save()} />
      <MachineRow title={m.scheduled_cancel()} disabled={busy} onPress={close} />
    </MachineGroup>
  </Pagina></KeyboardAvoidingView>;
}
const styles = StyleSheet.create((theme) => ({
  days: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[1] },
  day: { minHeight: 44, minWidth: 44, alignItems: 'center', justifyContent: 'center', borderWidth: 1, borderRadius: theme.base.radius.sm, paddingHorizontal: theme.base.space[2] },
  dayText: { fontSize: theme.base.text.xs },
}));
