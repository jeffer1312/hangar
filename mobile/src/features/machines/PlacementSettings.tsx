import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Stack } from 'expo-router';
import { ownUpdatableServers, placementLimits, placementLocalTz, placementProjectTitle, placementProjects, placementScheduleRule, placementStatus,
  readPlacementAll, savePlacementAll, type PlacementConfig, type PlacementMachine, type PlacementSaved } from '@hangar/core';
import { Pagina } from '../config/Pagina';
import { InfoNotice } from '../config/InfoNotice';
import { SectionCard } from '../config/SectionCard';
import { Segmented } from '../config/Segmented';
import { useSettingsColors } from '../config/colors';
import { ActionRow, PickerRow, PlainRow } from '../dream/rows';
import { MachineField, MachineGroup, MachineRow } from './MachineRows';
import { Notice } from './parts';
import { useServers } from '../../stores/servers';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

const UI_MODES = [
  { v: 'off', label: m.native_placement_mode_off() }, { v: 'suggest', label: m.native_placement_mode_suggest() }, { v: 'ask', label: m.native_placement_mode_ask() },
] as const;
const AGENT_MODES = [
  { v: 'off', label: m.native_placement_mode_off() }, { v: 'auto', label: m.native_placement_mode_auto() }, { v: 'ask', label: m.native_placement_mode_ask() },
] as const;
const DAYS = [m.native_placement_day_1, m.native_placement_day_2, m.native_placement_day_3, m.native_placement_day_4, m.native_placement_day_5,
  m.native_placement_day_6, m.native_placement_day_7];

type Limits = { mem: string; working: string; cpu: string };
const limitsText = (c: PlacementConfig): Limits => ({ mem: String(c.limits.mem_free_pct), working: String(c.limits.working), cpu: String(c.limits.cpu_pct) });

// Regras do balanceamento: lidas da máquina escolhida nas Configurações e gravadas em todas as próprias que responderam.
export default function PlacementSettings() {
  const c = useSettingsColors();
  const servers = useServers((s) => s.servers);
  const ready = useServers((s) => s.ready);
  const activeId = useServers((s) => s.activeId);
  const own = useMemo(() => ownUpdatableServers(servers), [servers]);
  const ownKey = own.map((s) => `${s.id}@${s.baseUrl}`).join('|');
  const hereId = own.find((s) => s.id === activeId)?.id ?? own[0]?.id;

  const [machines, setMachines] = useState<PlacementMachine[] | null>(null);
  const [draft, setDraft] = useState<PlacementConfig | null>(null);
  const [limits, setLimits] = useState<Limits>({ mem: '', working: '', cpu: '' });
  const [rule, setRule] = useState<{ project: string | null; server: string | null }>({ project: null, server: null });
  const [schedule, setSchedule] = useState({ days: [] as number[], from: '', to: '', tz: placementLocalTz(), avoid: null as string | null });
  const [saved, setSaved] = useState<PlacementSaved[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  // Só a resposta da última leitura ou gravação vale.
  const seq = useRef(0);

  const load = useCallback(() => {
    const n = ++seq.current;
    setMachines(null);
    setSaved(null);
    setSaving(false);
    void readPlacementAll(own).then((read) => {
      if (n !== seq.current) return;
      setMachines(read);
      const here = read.find((r) => r.serverId === hereId)?.read;
      const config = here?.state === 'ready' ? here.config : null;
      setDraft(config);
      if (config) setLimits(limitsText(config));
    });
  }, [own, hereId]);
  useEffect(() => { if (own.length) load(); }, [ownKey, hereId]);

  const labelOf = (ident: string) => {
    const serverId = machines?.find((r) => r.ident === ident)?.serverId;
    return own.find((s) => s.id === serverId)?.label ?? ident;
  };
  const known = (machines ?? []).filter((r) => r.ident).map((r) => ({ id: r.ident!, title: labelOf(r.ident!) }));
  const projects = placementProjects(machines ?? []);
  const here = machines?.find((r) => r.serverId === hereId) ?? null;
  const hereConfig = here?.read.state === 'ready' ? here.read.config : null;

  const edit = (change: (d: PlacementConfig) => PlacementConfig) => {
    if (saving || !draft) return;
    setDraft(change(draft));
    setError('');
  };
  const addRule = () => {
    const { project, server } = rule;
    if (!project || !server) { setError(m.native_placement_project_missing()); return; }
    // Um projeto tem uma máquina só: a regra nova troca a anterior.
    edit((d) => ({ ...d, project_rules: [...d.project_rules.filter((r) => r.project !== project), { project, server }] }));
    setRule({ project: null, server: null });
  };
  const addSchedule = () => {
    const check = placementScheduleRule(schedule.days, schedule.from, schedule.to, schedule.tz, schedule.avoid);
    if (!check.ok) { setError(check.error); return; }
    edit((d) => ({ ...d, schedule_rules: [...d.schedule_rules, check.value] }));
    setSchedule((s) => ({ ...s, days: [], from: '', to: '', avoid: null }));
  };
  const toggleDay = (day: number) => {
    haptic.selection();
    setSchedule((s) => ({ ...s, days: s.days.includes(day) ? s.days.filter((d) => d !== day) : [...s.days, day] }));
  };

  const save = () => {
    if (!draft || !machines || saving) return;
    const checked = placementLimits(limits.mem, limits.working, limits.cpu);
    if (!checked.ok) { setError(checked.error); haptic.error(); return; }
    const clone_dirs = Object.fromEntries(Object.entries(draft.clone_dirs).map(([k, v]) => [k, v.trim()]).filter(([, v]) => v));
    const config: PlacementConfig = { ...draft, limits: checked.value, clone_dirs };
    const targets = own.filter((s) => machines.find((r) => r.serverId === s.id)?.read.state === 'ready');
    const n = ++seq.current;
    setDraft(config);
    setError('');
    setSaved(null);
    setSaving(true);
    void savePlacementAll(targets, config).then((results) => {
      if (n !== seq.current) return;
      setSaving(false);
      setSaved(results);
      setMachines((list) => list?.map((r) => {
        const result = results.find((x) => x.serverId === r.serverId);
        return result?.ok ? { ...r, read: { state: 'ready', config: result.config ?? config } } : r;
      }) ?? null);
      if (results.every((r) => r.ok)) haptic.success();
      else haptic.error();
    });
  };

  let body: ReactNode;
  if (!ready || (own.length > 0 && machines === null)) body = <InfoNotice text={m.native_server_loading()} />;
  else if (own.length === 0) body = <InfoNotice text={m.maquinas_vazio()} />;
  else if (!here || here.read.state !== 'ready' || !draft) {
    body = <MachineGroup>
      <MachineRow title={here?.read.state === 'old' ? m.native_placement_old_here()
        : m.native_placement_read_failed({ motivo: here?.read.state === 'failed' ? here.read.error : '' })} />
      <MachineRow title={m.mobile_machines_retry()} action onPress={load} />
    </MachineGroup>;
  } else {
    const dayList = (days: number[]) => days.length ? days.map((d) => DAYS[d - 1]?.() ?? String(d)).join(', ') : m.native_placement_every_day();
    body = <>
      <SectionCard icon="Workflow" title={m.native_placement_when()} subtitle={m.native_placement_when_help()}>
        <ModeRow title={m.native_placement_ui_mode()} help={m.native_placement_ui_mode_help()}>
          <Segmented options={UI_MODES} value={draft.ui_mode} label={m.native_placement_ui_mode()} disabled={saving}
            onChange={(v) => edit((d) => ({ ...d, ui_mode: v }))} />
        </ModeRow>
        <ModeRow title={m.native_placement_agent_mode()} help={m.native_placement_agent_mode_help()}>
          <Segmented options={AGENT_MODES} value={draft.agent_mode} label={m.native_placement_agent_mode()} disabled={saving}
            onChange={(v) => edit((d) => ({ ...d, agent_mode: v }))} />
        </ModeRow>
      </SectionCard>

      <SectionCard icon="Gauge" title={m.native_placement_limits()} subtitle={m.native_placement_limits_help()}>
        <MachineField label={`${m.native_placement_limit_memory()} (%)`} accessibilityHint={m.native_placement_limit_memory_help()} value={limits.mem}
          onChangeText={(mem) => setLimits((l) => ({ ...l, mem }))} keyboardType="number-pad" maxLength={3} editable={!saving} />
        <MachineField label={m.native_placement_limit_working()} accessibilityHint={m.native_placement_limit_working_help()} value={limits.working}
          onChangeText={(working) => setLimits((l) => ({ ...l, working }))} keyboardType="number-pad" maxLength={3} editable={!saving} />
        <MachineField label={`${m.native_placement_limit_cpu()} (%)`} accessibilityHint={m.native_placement_limit_cpu_help()} value={limits.cpu}
          onChangeText={(cpu) => setLimits((l) => ({ ...l, cpu }))} keyboardType="number-pad" maxLength={3} editable={!saving} />
      </SectionCard>

      <SectionCard icon="FolderGit2" title={m.native_placement_projects()} subtitle={m.native_placement_projects_help()}>
        {draft.project_rules.length ? draft.project_rules.map((r) => <ActionRow key={r.project} title={placementProjectTitle(r.project)}
          description={m.native_placement_project_to({ maquina: labelOf(r.server) })} action={m.native_placement_remove()} disabled={saving}
          onAction={() => edit((d) => ({ ...d, project_rules: d.project_rules.filter((x) => x.project !== r.project) }))} />)
          : <PlainRow title={m.native_placement_projects_empty()} />}
        <PickerRow title={m.native_placement_project_pick()} disabled={saving || !projects.length}
          value={rule.project ? placementProjectTitle(rule.project) : projects.length ? '—' : m.native_placement_project_none_known()}
          options={projects.map((p) => ({ id: p, title: placementProjectTitle(p), selected: p === rule.project }))}
          onPick={(project) => setRule((r) => ({ ...r, project }))} />
        <PickerRow title={m.native_placement_machine_pick()} disabled={saving} value={rule.server ? labelOf(rule.server) : '—'}
          options={known.map((k) => ({ ...k, selected: k.id === rule.server }))} onPick={(server) => setRule((r) => ({ ...r, server }))} />
        <MachineRow title={m.native_placement_add()} action disabled={saving} onPress={addRule} />
      </SectionCard>

      <SectionCard icon="CalendarClock" title={m.native_placement_schedules()} subtitle={m.native_placement_schedules_help()}>
        {draft.schedule_rules.length ? draft.schedule_rules.map((r, i) => <ActionRow key={`${i}:${r.avoid}:${r.from}`}
          title={m.native_placement_schedule_row({ dias: dayList(r.days), de: r.from, ate: r.to })}
          description={m.native_placement_schedule_avoid({ maquina: labelOf(r.avoid), fuso: r.tz })} action={m.native_placement_remove()} disabled={saving}
          onAction={() => edit((d) => ({ ...d, schedule_rules: d.schedule_rules.filter((_, j) => j !== i) }))} />)
          : <PlainRow title={m.native_placement_schedules_empty()} />}
        <View style={[styles.stack, { borderTopColor: c.border }]}>
          <Text style={[styles.title, { color: c.text }]}>{m.native_placement_days()}</Text>
          <View style={styles.days} accessibilityRole="none">
            {DAYS.map((name, i) => {
              const on = schedule.days.includes(i + 1);
              return <Pressable key={i} onPress={() => toggleDay(i + 1)} disabled={saving} accessibilityRole="checkbox" accessibilityLabel={name()}
                accessibilityState={{ checked: on, disabled: saving }}
                style={[styles.day, { borderColor: c.borderStrong, backgroundColor: on ? c.accentDim : 'transparent' }]}>
                <Text style={[styles.dayText, { color: on ? c.accentText : c.muted }]}>{name()}</Text>
              </Pressable>;
            })}
          </View>
          <Text style={[styles.help, { color: c.muted }]}>{m.native_placement_days_help()}</Text>
        </View>
        <MachineField label={m.native_placement_from()} value={schedule.from} placeholder="09:00" maxLength={5} editable={!saving}
          keyboardType="numbers-and-punctuation" onChangeText={(from) => setSchedule((s) => ({ ...s, from }))} />
        <MachineField label={m.native_placement_to()} value={schedule.to} placeholder="18:00" maxLength={5} editable={!saving}
          keyboardType="numbers-and-punctuation" onChangeText={(to) => setSchedule((s) => ({ ...s, to }))} />
        <MachineField label={m.native_placement_tz()} value={schedule.tz} placeholder="America/Sao_Paulo" editable={!saving}
          onChangeText={(tz) => setSchedule((s) => ({ ...s, tz }))} />
        <PickerRow title={m.native_placement_machine_avoid()} disabled={saving} value={schedule.avoid ? labelOf(schedule.avoid) : '—'}
          options={known.map((k) => ({ ...k, selected: k.id === schedule.avoid }))} onPick={(avoid) => setSchedule((s) => ({ ...s, avoid }))} />
        <MachineRow title={m.native_placement_add()} action disabled={saving} onPress={addSchedule} />
      </SectionCard>

      <SectionCard icon="FolderDown" title={m.native_placement_clone()} subtitle={m.native_placement_clone_help()}>
        {known.length ? known.map((k) => {
          const auto = machines?.find((r) => r.ident === k.id)?.facts?.clone_dir;
          return <MachineField key={k.id} stacked label={m.native_placement_clone_for({ maquina: k.title })} value={draft.clone_dirs[k.id] ?? ''}
            placeholder={auto ? m.native_placement_clone_auto({ dir: auto }) : m.native_placement_clone_auto_unknown()} editable={!saving}
            onChangeText={(dir) => edit((d) => ({ ...d, clone_dirs: { ...d.clone_dirs, [k.id]: dir } }))} />;
        }) : <PlainRow title={m.native_placement_clone_empty()} />}
      </SectionCard>

      <SectionCard icon="Server" title={m.native_placement_machines()} subtitle={m.native_placement_machines_help()}>
        {(machines ?? []).map((r) => {
          const server = own.find((s) => s.id === r.serverId);
          const label = server?.label ?? r.serverId;
          const status = placementStatus(r.read, hereConfig);
          const statusText = status === 'old' ? m.native_placement_machine_old() : status === 'silent'
            ? m.native_placement_machine_silent({ motivo: r.read.state === 'failed' ? r.read.error : '' })
            : status === 'same' ? m.native_placement_machine_same() : m.native_placement_machine_different();
          const result = saved?.find((x) => x.serverId === r.serverId);
          return <MachineRow key={r.serverId} title={r.serverId === hereId ? m.native_placement_machine_this({ maquina: label }) : label}
            description={[statusText, r.facts?.version ? m.native_placement_machine_version({ versao: r.facts.version }) : ''].filter(Boolean).join(' · ')}
            value={result ? result.ok ? m.native_placement_machine_saved() : m.native_placement_machine_save_failed({ motivo: result.error ?? '' }) : undefined}
            valueTone={result ? result.ok ? 'success' : 'error' : undefined} />;
        })}
      </SectionCard>

      {error ? <Notice text={error} error /> : null}
      {saved?.length && saved.every((r) => r.ok) ? <Notice text={m.native_placement_saved_all()} /> : null}
      <MachineGroup>
        <MachineRow title={saving ? m.native_server_saving() : m.native_placement_save_all()} action disabled={saving} onPress={save} />
      </MachineGroup>
    </>;
  }

  return (
    <Pagina>
      <Stack.Screen options={{ title: m.native_settings_page_placement() }} />
      <InfoNotice text={m.native_placement_intro()} />
      {body}
    </Pagina>
  );
}

function ModeRow({ title, help, children }: { title: string; help: string; children: ReactNode }) {
  const c = useSettingsColors();
  return <View style={[styles.stack, { borderTopColor: c.border }]}>
    <Text style={[styles.title, { color: c.text }]}>{title}</Text>
    {children}
    <Text style={[styles.help, { color: c.muted }]}>{help}</Text>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  stack: { gap: theme.base.space[2], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], borderTopWidth: 1 },
  title: { fontSize: theme.base.text.base },
  help: { fontSize: theme.base.text.xs, lineHeight: 19 },
  days: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[1.5] },
  day: { minWidth: 44, minHeight: 44, paddingHorizontal: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.sm, alignItems: 'center', justifyContent: 'center' },
  dayText: { fontSize: theme.base.text.sm },
}));
