import { MenuView } from '@react-native-menu/menu';
import { Pressable, Switch, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { basename, providerName, intlLocale, type ScheduledTask, type TaskSchedule } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

export const taskWeekdays = () => [m.scheduled_weekday_sun(), m.scheduled_weekday_mon(), m.scheduled_weekday_tue(), m.scheduled_weekday_wed(), m.scheduled_weekday_thu(), m.scheduled_weekday_fri(), m.scheduled_weekday_sat()];
export const taskTime = (at: string, timezone: string) => new Intl.DateTimeFormat(intlLocale(), { dateStyle: 'short', timeStyle: 'short', timeZone: timezone }).format(new Date(at));
export function scheduleText(schedule: TaskSchedule, timezone: string): string {
  if (schedule.kind === 'once') return m.scheduled_schedule_once({ time: taskTime(schedule.at, timezone) });
  if (schedule.kind === 'interval') return m.scheduled_schedule_interval({ minutes: schedule.minutes });
  const days = taskWeekdays();
  return m.scheduled_schedule_weekly({ days: schedule.weekdays.length === 7 ? m.scheduled_every_day() : schedule.weekdays.map((d) => days[d]).join(', '), time: schedule.time });
}
export const taskStatus = (task: ScheduledTask) => ({ never: m.scheduled_status_never, dispatching: m.scheduled_status_dispatching,
  sent: m.scheduled_status_sent, failed: m.scheduled_status_failed, uncertain: m.scheduled_status_uncertain })[task.last_status]();

/** A lista e a novidade usam a mesma linha, inclusive os estados de envio. */
export function TaskRow({ task, timezone, disabled, onEdit, onToggle, onRun, onDelete, onOpen }: {
  task: ScheduledTask; timezone: string; disabled?: boolean; onEdit?: () => void; onToggle?: () => void; onRun?: () => void; onDelete?: () => void; onOpen?: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const failed = task.last_status === 'failed' || task.last_status === 'uncertain';
  return <View style={styles.row}>
    <View style={styles.heading}>
      <Pressable accessibilityRole="button" accessibilityLabel={`${m.scheduled_edit()}: ${task.name}`} disabled={disabled || !onEdit} onPress={onEdit} style={styles.nameButton}>
        <Text style={[styles.name, { color: c.text }]}>{task.name}</Text>
      </Pressable>
      <Switch accessibilityLabel={task.enabled ? m.scheduled_pause() : m.scheduled_activate()} value={task.enabled} disabled={disabled || !onToggle} onValueChange={onToggle} />
      <MenuView title={task.name} actions={[
        { id: 'edit', title: m.scheduled_edit(), attributes: { disabled: !!disabled || !onEdit } },
        { id: 'run', title: m.scheduled_run_now(), attributes: { disabled: !!disabled || !onRun } },
        { id: 'delete', title: m.scheduled_delete(), attributes: { disabled: !!disabled || !onDelete, destructive: true } },
      ]} onPressAction={({ nativeEvent }) => {
        if (disabled) return;
        if (nativeEvent.event === 'edit') onEdit?.();
        else if (nativeEvent.event === 'run') onRun?.();
        else if (nativeEvent.event === 'delete') onDelete?.();
      }}>
        <Pressable accessibilityRole="button" accessibilityLabel={`${m.scheduled_actions()}: ${task.name}`} disabled={disabled} style={styles.action}>
          <Icon name="Ellipsis" size={21} color={c.muted} />
        </Pressable>
      </MenuView>
    </View>
    <Text style={[styles.prompt, { color: c.muted }]} numberOfLines={2}>{task.prompt}</Text>
    <View style={styles.meta}><Icon name="Clock" size={14} color={c.faint} /><Text style={[styles.detail, { color: c.muted }]}>{scheduleText(task.schedule, timezone)}</Text></View>
    <Text style={[styles.detail, { color: c.faint }]}>{task.target.kind === 'session' ? task.target.name : [task.target.launch.cwd ? basename(task.target.launch.cwd) : m.scheduled_target_new(), task.target.launch.provider ? providerName(task.target.launch.provider) : ''].filter(Boolean).join(' · ')}</Text>
    <Text style={[styles.detail, { color: task.enabled ? c.accentText : c.faint }]}>{!task.enabled ? m.scheduled_status_paused() : task.next_run_at ? m.scheduled_next({ time: taskTime(task.next_run_at, timezone) }) : m.scheduled_not_scheduled()}</Text>
    <Text style={[styles.detail, { color: failed ? theme.tokens.status.error : c.muted }]}>{taskStatus(task)}{task.last_run_at ? ` · ${m.scheduled_last({ time: taskTime(task.last_run_at, timezone) })}` : ''}</Text>
    {task.last_error ? <Text selectable style={[styles.detail, { color: theme.tokens.status.error }]}>{task.last_error}</Text> : null}
    {task.last_session ? <Pressable disabled={!onOpen || disabled} onPress={onOpen} accessibilityRole="button" style={styles.open}>
      <Text style={[styles.detail, { color: c.accentText }]}>{m.scheduled_last_session()}</Text><Icon name="ExternalLink" size={14} color={c.accentText} />
    </Pressable> : null}
  </View>;
}
const styles = StyleSheet.create((theme) => ({
  row: { paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[3], gap: theme.base.space[1.5], borderTopWidth: 1, borderColor: theme.tokens.border.default },
  heading: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  nameButton: { flex: 1, minHeight: 44, justifyContent: 'center' },
  name: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  action: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  prompt: { fontSize: theme.base.text.sm }, detail: { fontSize: theme.base.text.xs, flexShrink: 1 },
  meta: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5] },
  open: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], alignSelf: 'flex-start' },
}));
