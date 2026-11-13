import { useState } from 'react';
import { RefreshControl } from 'react-native';
import { Stack, useRouter } from 'expo-router';
import { dismissDreamSuggestion, dreamErrorText, dreamLastText, dreamMonthText, parseDreamCap, putDreamConfig, startDream, validDreamHour,
  type DreamConfigPatch } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { SectionCard } from '../../src/features/config/SectionCard';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { ActionRow, FieldRow, PickerRow, SwitchRow, TapRow } from '../../src/features/dream/rows';
import { useDream } from '../../src/features/dream/useDream';
import { toast } from '../../src/ui/Toast';
import * as m from '../../src/paraglide/messages';

export default function Memoria() {
  const router = useRouter();
  const d = useDream();
  const { config, status, dreamer, servers, configs, busy } = d;
  // Servidores que não gravaram a última escolha de máquina: a escolha vale em todos.
  const [unconfirmed, setUnconfirmed] = useState<string[]>([]);
  const put = (patch: DreamConfigPatch) => { if (dreamer) void d.act(() => putDreamConfig(dreamer, patch)); };
  const chooseMachine = (serverId: string) => {
    const machine = configs[serverId]?.this_machine;
    if (!machine) return;
    void d.act(async () => {
      const results = await Promise.allSettled(servers.map((s) => putDreamConfig(s, { machine })));
      const failed = servers.filter((_, i) => results[i].status === 'rejected').map((s) => m.dream_machine_unconfirmed({ server: s.label }));
      setUnconfirmed(failed);
      return failed.length ? failed.join('\n') : undefined;
    });
  };
  const cap = (text: string, field: 'project_cap_usd' | 'month_cap_usd') => {
    const value = parseDreamCap(text);
    // Só o teto do mês aceita vazio (sem teto).
    if (value === undefined || (value === null && field === 'project_cap_usd')) { toast.erro(m.dream_invalid_cap()); return false; }
    put(value !== null && field === 'project_cap_usd' ? { project_cap_usd: value } : { month_cap_usd: value });
    return true;
  };

  return <Pagina refreshControl={<RefreshControl refreshing={d.loading && !!status} onRefresh={() => void d.reload()} />}>
    <Stack.Screen options={{ title: m.dream_title() }} />
    <MachineHelp>{m.dream_lead()}</MachineHelp>
    {d.error ? <><Notice text={d.error} error /><MachineGroup>
      <MachineRow title={m.lista_tentar_novamente()} action disabled={d.loading} onPress={() => void d.reload()} />
    </MachineGroup></> : null}
    {!status && d.loading ? <InfoNotice text={m.comum_carregando()} /> : null}
    {config && status ? <>
      <MachineGroup>
        <SwitchRow title={m.dream_enabled()} description={m.dream_enabled_hint()} value={config.enabled} disabled={busy} onChange={(enabled) => put({ enabled })} />
        <SwitchRow title={m.dream_user_memory()} description={m.dream_user_memory_hint()} value={config.user_memory} disabled={busy}
          onChange={(user_memory) => put({ user_memory })} />
        <PickerRow title={m.dream_machine()} description={m.dream_machine_hint()} disabled={busy}
          value={config.machine ? d.labelOf(config.machine) : '—'} error={unconfirmed.join('\n') || undefined}
          options={servers.map((s) => ({ id: s.id, title: s.label, selected: !!config.machine && configs[s.id]?.this_machine === config.machine, disabled: !configs[s.id] }))}
          onPick={chooseMachine} />
        <FieldRow key={config.api_key} title={m.dream_key()} description={m.dream_key_hint()} value="" placeholder={config.api_key || '—'} secureTextEntry
          disabled={busy} onCommit={(text) => { if (text.trim()) put({ api_key: text.trim() }); return true; }} />
        <FieldRow title={m.dream_hour()} value={config.hour} keyboardType="numbers-and-punctuation" disabled={busy}
          onCommit={(text) => { if (!validDreamHour(text.trim())) { toast.erro(m.dream_invalid_hour()); return false; } put({ hour: text.trim() }); return true; }} />
        <FieldRow title={m.dream_project_cap()} description={m.dream_project_cap_hint()} value={String(config.project_cap_usd)} keyboardType="decimal-pad"
          disabled={busy} onCommit={(text) => cap(text, 'project_cap_usd')} />
        <FieldRow title={m.dream_month_cap()} description={m.dream_month_cap_hint()} value={config.month_cap_usd === null ? '' : String(config.month_cap_usd)}
          keyboardType="decimal-pad" disabled={busy} onCommit={(text) => cap(text, 'month_cap_usd')} />
        <MachineRow title={m.dream_month_spent()} description={dreamMonthText(status.month, config.month_cap_usd)} />
      </MachineGroup>
      <MachineGroup>
        <MachineRow title={status.running ? m.dream_running() : m.dream_run_now()} action disabled={busy || !!status.running || !dreamer}
          onPress={() => { if (dreamer) void d.act(() => startDream(dreamer)); }} />
      </MachineGroup>
      {status.projects.length ? <SectionCard icon="Folder" title={m.dream_projects()}>
        {status.projects.map((p) => <TapRow key={p.id} title={p.name}
          description={`${p.machines.map(d.labelOf).join(', ')} · ${m.dream_new_sessions({ n: p.new_sessions })}\n${dreamLastText(p.last_dream_at, p.last_cost_usd)}`}
          error={p.last_error ? dreamErrorText(p.last_error) : undefined}
          onPress={() => router.push({ pathname: '/config/memoria-project', params: { id: p.id } } as never)} />)}
      </SectionCard> : <InfoNotice text={m.dream_projects_empty()} />}
      {status.suggestions.length ? <SectionCard icon="FileText" title={m.dream_suggestions()} subtitle={m.dream_suggestions_hint()}>
        {status.suggestions.map((s, index) => <ActionRow key={`${index}:${s.text}`} title={s.text}
          description={s.conflicts_with ? m.dream_suggestion_conflict({ line: s.conflicts_with }) : undefined}
          action={m.dream_dismiss()} disabled={busy || !dreamer} onAction={() => { if (dreamer) void d.act(() => dismissDreamSuggestion(dreamer, index)); }} />)}
      </SectionCard> : null}
    </> : null}
  </Pagina>;
}
