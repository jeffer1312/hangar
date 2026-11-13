import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, RefreshControl } from 'react-native';
import { Stack, useRouter } from 'expo-router';
import { createScheduledTask, deleteScheduledTask, getScheduledTasks, reuseUnchangedTasks, runScheduledTask, setScheduledTaskEnabled, updateScheduledTask,
  type ScheduledTask, type ScheduledTaskInput, type ScheduledTasksList } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { Segmented } from '../../src/features/config/Segmented';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { SettingRow } from '../../src/features/create/SettingRow';
import { TaskRow } from '../../src/features/scheduledTasks/TaskRow';
import { TaskEditor } from '../../src/features/scheduledTasks/TaskEditor';
import { scheduledError } from '../../src/features/scheduledTasks/errors';
import { newId } from '../../src/net/deviceIdentity';
import { ownServers, useServers } from '../../src/stores/servers';
import { haptic } from '../../src/ui/haptics';
import * as m from '../../src/paraglide/messages';

type Filter = 'all' | 'on' | 'off';
// `requestId` vale para toda a vida do editor de criação: a nova tentativa após resposta perdida não duplica.
type Editing = { task?: ScheduledTask; requestId?: string };
export default function ScheduledTasks() {
  const router = useRouter();
  const server = useServers((s) => s.active());
  const servers = useServers((s) => s.servers);
  const [data, setData] = useState<ScheduledTasksList | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const operation = useRef(false);
  const generation = useRef(0);
  const activeServer = useRef(server);
  const [filter, setFilter] = useState<Filter>('all');
  const [editor, setEditor] = useState<Editing | null>(null);
  const [notice, setNotice] = useState('');
  const [clockOffset, setClockOffset] = useState(0);
  const install = useCallback((value: ScheduledTasksList) => {
    setData((prev) => ({ ...value, tasks: reuseUnchangedTasks(prev?.tasks, value.tasks) }));
    setClockOffset(Date.parse(value.server_time) - Date.now());
  }, []);
  const load = useCallback(async () => {
    if (!server || server.invite || activeServer.current !== server) return;
    const current = ++generation.current;
    setLoading(true); setError('');
    try { const value = await getScheduledTasks(server); if (current === generation.current && activeServer.current === server) install(value); }
    catch (e) { if (current === generation.current && activeServer.current === server) setError(scheduledError(e)); }
    finally { if (current === generation.current && activeServer.current === server) setLoading(false); }
  }, [server, install]);
  // Só a renderização confirmada troca o servidor; vem antes do efeito que carrega.
  useEffect(() => { activeServer.current = server; }, [server]);
  useEffect(() => { setData(null); setEditor(null); setNotice(''); void load(); return () => { generation.current++; }; }, [load]);
  const act = useCallback(async (action: () => Promise<unknown>, success?: string) => {
    if (operation.current) return;
    operation.current = true; setBusy(true); setError(''); setNotice('');
    try { await action(); if (activeServer.current !== server) return; if (success) setNotice(success); haptic.success(); await load(); }
    catch (e) { if (activeServer.current === server) { setError(scheduledError(e)); haptic.error(); } }
    finally { operation.current = false; setBusy(false); }
  }, [server, load]);
  const editTask = useCallback((task: ScheduledTask) => setEditor({ task }), []);
  const runTask = useCallback((task: ScheduledTask) => { if (server) void act(() => runScheduledTask(server, task.id), m.scheduled_run_started()); }, [server, act]);
  const toggleTask = useCallback((task: ScheduledTask) => { if (server) void act(() => setScheduledTaskEnabled(server, task.id, task.revision, !task.enabled)); }, [server, act]);
  const removeTask = useCallback((task: ScheduledTask) => { if (server) Alert.alert(m.scheduled_delete_title(), m.scheduled_delete_body(), [
    { text: m.scheduled_keep(), style: 'cancel' },
    { text: m.scheduled_confirm_delete(), style: 'destructive', onPress: () => void act(() => deleteScheduledTask(server, task.id, task.revision), m.scheduled_deleted()) },
  ]); }, [server, act]);
  const openTask = useCallback((task: ScheduledTask) => {
    if (server && task.last_session) router.push(`/s/${encodeURIComponent(server.id)}/${encodeURIComponent(task.last_session)}` as never);
  }, [server, router]);
  if (!server || server.invite) return <Pagina><Stack.Screen options={{ title: m.scheduled_title() }} /><InfoNotice text={m.scheduled_load_failed()} /></Pagina>;
  // A resposta que chega depois de trocar de servidor ou de editor não mexe no que está aberto agora.
  const save = async (input: ScheduledTaskInput) => {
    const opened = editor; const target = server;
    if (opened?.task) await updateScheduledTask(target, opened.task.id, opened.task.revision, input);
    else await createScheduledTask(target, { ...input, request_id: opened?.requestId });
    if (activeServer.current !== target) return;
    setEditor((cur) => (cur === opened ? null : cur)); setNotice(opened?.task ? m.scheduled_saved() : m.scheduled_created()); void load();
  };
  if (editor && data) return <TaskEditor key={editor.task ? `${editor.task.id}:${editor.task.revision}` : 'new'} server={server} timezone={data.timezone} clockOffset={clockOffset} task={editor.task}
    onSave={save} onClose={() => setEditor(null)} onReload={async () => {
      const opened = editor; const target = server;
      const fresh = await getScheduledTasks(target);
      if (activeServer.current !== target) return;
      install(fresh);
      const task = fresh.tasks.find((t) => t.id === opened.task?.id);
      setEditor((cur) => (cur === opened ? (task ? { task } : null) : cur));
    }} />;
  const tasks = data?.tasks.filter((task) => filter === 'all' || (filter === 'on' ? task.enabled : !task.enabled)) ?? [];
  return <Pagina refreshControl={<RefreshControl refreshing={loading && !!data} onRefresh={() => void load()} />}>
    <Stack.Screen options={{ title: m.scheduled_title() }} />
    <MachineHelp>{m.scheduled_intro()}</MachineHelp>
    <MachineGroup><SettingRow icon="Server" label={m.scheduled_server()} value={server.label} disabled={busy} actions={ownServers(servers).map((s) => ({ id: s.id, title: s.label, state: s.id === server.id ? 'on' : 'off' }))}
      onAction={(id) => useServers.getState().setActive(id)} /></MachineGroup>
    {data ? <MachineHelp>{m.scheduled_timezone({ timezone: data.timezone })}</MachineHelp> : null}
    <MachineGroup><MachineRow title={m.scheduled_new()} action disabled={!data || busy} onPress={() => { setNotice(''); setEditor({ requestId: newId() }); }} /></MachineGroup>
    <Segmented<Filter> label={m.scheduled_title()} value={filter} minHeight={44} onChange={setFilter} options={[
      { v: 'all', label: m.scheduled_all() }, { v: 'on', label: m.scheduled_active() }, { v: 'off', label: m.scheduled_paused() },
    ]} />
    {notice ? <Notice text={notice} /> : null}
    {error ? <><Notice text={error} error /><MachineGroup><MachineRow title={m.scheduled_retry()} action disabled={busy || loading} onPress={() => void load()} /></MachineGroup></> : null}
    {loading && !data ? <InfoNotice text={m.scheduled_loading()} /> : data && !tasks.length ? <InfoNotice text={`${m.scheduled_empty()} · ${m.scheduled_empty_hint()}`} /> : null}
    {tasks.length && data ? <MachineGroup>{tasks.map((task) => <TaskRow key={task.id} task={task} timezone={data.timezone} disabled={busy}
      onEdit={editTask} onRun={runTask} onDelete={removeTask} onToggle={toggleTask} onOpen={openTask} />)}</MachineGroup> : null}
    <MachineHelp>{m.scheduled_runs_closed()}</MachineHelp>
  </Pagina>;
}
