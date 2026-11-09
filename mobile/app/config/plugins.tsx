import { useEffect, useMemo, useState } from 'react';
import { Alert, AppState, Switch } from 'react-native';
import { Stack, useRouter } from 'expo-router';
import { addMarketplaceForServer, intlLocale, setMarketplaceAutoUpdateForServer, type InstalledPlugin, type PluginMarketplace, type Server } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { SectionTabs } from '../../src/features/config/SectionTabs';
import { Segmented } from '../../src/features/config/Segmented';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { PluginRow, PluginsField, ReopenBanner, RowButton, checkedLabel, shortDate, useLiveClaude } from '../../src/features/plugins/parts';
import { runMarketplaceAction, runPluginAction, usePlugins } from '../../src/features/plugins/store';
import { useServers } from '../../src/stores/servers';
import { haptic } from '../../src/ui/haptics';
import * as m from '../../src/paraglide/messages';

type Tab = 'installed' | 'discover' | 'marketplaces';
type Filter = 'all' | 'on' | 'off' | 'up';

const fold = (s: string) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
const matches = (q: string, ...parts: Array<string | null | undefined>) => !q || fold(parts.filter(Boolean).join(' ')).includes(q);
// O servidor confere de novo; aqui só barra o formato antes de mandar.
const SOURCE = /^[\w.-]+\/[\w.-]+$|^https:\/\/\S+$|^\/.+$/;

export default function Plugins() {
  const server = useServers((s) => s.active());
  const state = usePlugins((s) => s.state);
  const loading = usePlugins((s) => s.loading);
  const error = usePlugins((s) => s.error);
  const busy = usePlugins((s) => s.busy);
  const [tab, setTab] = useState<Tab>('installed');

  useEffect(() => {
    if (!server) return;
    usePlugins.getState().reset(server);
    void usePlugins.getState().load(server);
  }, [server?.id, server?.baseUrl, server?.token]);

  // A rota fica montada no fundo: ao voltar, o terminal pode ter instalado ou atualizado algo.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (st) => {
      const current = useServers.getState().active();
      if (st === 'active' && current) void usePlugins.getState().load(current);
    });
    return () => sub.remove();
  }, []);

  return (
    <Pagina>
      <Stack.Screen options={{ title: m.plugins_title() }} />
      <MachineHelp>{m.plugins_subtitle()}</MachineHelp>
      {!server ? null
        : !state && loading ? <InfoNotice text={m.plugins_loading()} />
        : !state && error ? <>
          <Notice text={error} error />
          <MachineGroup><MachineRow title={m.plugins_retry()} action onPress={() => void usePlugins.getState().load(server)} /></MachineGroup>
        </>
        : state ? <>
          <SectionTabs<Tab> current={tab} onPick={(k) => { haptic.selection(); setTab(k); }} tabs={[
            { key: 'installed', label: m.plugins_tab_installed() },
            { key: 'discover', label: m.plugins_tab_discover() },
            { key: 'marketplaces', label: m.plugins_tab_marketplaces() },
          ]} />
          <MachineGroup>
            <MachineRow title={busy === 'check' ? m.plugins_checking() : m.plugins_check()} value={checkedLabel(state.checked_at)}
              action disabled={!!busy} onPress={() => void usePlugins.getState().check(server)} />
          </MachineGroup>
          <ReopenBanner server={server} />
          {error ? <Notice text={error} error /> : null}
          {tab === 'installed' ? <Installed server={server} plugins={state.plugins} />
            : tab === 'discover' ? <Discover server={server} installed={state.plugins} />
            : <Marketplaces server={server} marketplaces={state.marketplaces} />}
        </> : null}
    </Pagina>
  );
}

function Installed({ server, plugins }: { server: Server; plugins: InstalledPlugin[] }) {
  const router = useRouter();
  const live = useLiveClaude(server);
  const busy = usePlugins((s) => s.busy);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const updates = plugins.filter((p) => p.available_version).length;
  const q = fold(query.trim());
  const shown = plugins.filter((p) => matches(q, p.name, p.description)
    && (filter === 'all' || (filter === 'on' && p.enabled) || (filter === 'off' && !p.enabled) || (filter === 'up' && !!p.available_version)));
  return <>
    <PluginsField icon="Search" value={query} onChangeText={setQuery} placeholder={m.plugins_filter_placeholder()} returnKeyType="search" />
    <Segmented<Filter> label={m.plugins_filter_placeholder()} value={filter} minHeight={44} onChange={(v) => { haptic.selection(); setFilter(v); }} options={[
      { v: 'all', label: m.plugins_filter_all() },
      { v: 'on', label: m.plugins_filter_enabled() },
      { v: 'off', label: m.plugins_filter_disabled() },
      { v: 'up', label: m.plugins_filter_updates({ n: updates }) },
    ]} />
    {!plugins.length ? <InfoNotice text={m.plugins_empty_installed()} />
      : !shown.length ? <InfoNotice text={filter === 'up' && !q ? m.plugins_all_latest() : m.plugins_empty_filter()} />
      : <MachineGroup>
        {shown.map((p) => {
          const updating = busy === `${p.id}:update`;
          return <PluginRow key={p.id} title={p.name} dimmed={!p.enabled}
            pill={p.available_version ? m.plugins_available({ version: p.available_version }) : undefined}
            meta={m.plugins_row_meta({ version: p.version, date: shortDate(p.last_updated), marketplace: p.marketplace })}
            description={p.description}
            onPress={() => router.push({ pathname: '/config/plugin', params: { id: p.id } })}
            trailing={<>
              {p.available_version ? <RowButton label={updating ? m.plugins_updating() : m.plugins_update()} disabled={!!busy}
                onPress={() => void runPluginAction(server, p, 'update', live.length)} /> : null}
              <Switch value={p.enabled} disabled={!!busy} accessibilityLabel={p.name}
                onValueChange={(on) => void runPluginAction(server, p, on ? 'enable' : 'disable', live.length)} />
            </>} />;
        })}
      </MachineGroup>}
  </>;
}

function Discover({ server, installed }: { server: Server; installed: InstalledPlugin[] }) {
  const live = useLiveClaude(server);
  const available = usePlugins((s) => s.available);
  const loading = usePlugins((s) => s.availableLoading);
  const error = usePlugins((s) => s.availableError);
  const busy = usePlugins((s) => s.busy);
  const [query, setQuery] = useState('');
  useEffect(() => {
    const st = usePlugins.getState();
    if (st.available === null && !st.availableLoading && !st.availableError) void st.loadAvailable(server);
  }, [server, available]);
  const ids = useMemo(() => new Set(installed.map((p) => p.id)), [installed]);
  const q = fold(query.trim());
  const shown = (available ?? []).filter((a) => matches(q, a.name, a.description));
  if (!available) {
    return loading ? <InfoNotice text={m.plugins_loading()} />
      : error ? <>
        <Notice text={error} error />
        <MachineGroup><MachineRow title={m.plugins_retry()} action onPress={() => void usePlugins.getState().loadAvailable(server)} /></MachineGroup>
      </> : null;
  }
  return <>
    <PluginsField icon="Search" value={query} onChangeText={setQuery} returnKeyType="search"
      placeholder={m.plugins_discover_placeholder({ n: available.length.toLocaleString(intlLocale()) })} />
    {!available.length ? <InfoNotice text={m.plugins_discover_none()} />
      : !shown.length ? <InfoNotice text={m.plugins_discover_empty({ query: query.trim() })} />
      : <MachineGroup>
        {shown.map((a) => {
          const installing = busy === `${a.id}:install`;
          return <PluginRow key={a.id} title={a.name} description={a.description}
            meta={[a.version, a.marketplace].filter(Boolean).join(' · ')}
            trailing={ids.has(a.id) ? <MachineHelp>{m.plugins_installed()}</MachineHelp>
              : <RowButton primary label={installing ? m.plugins_installing() : m.plugins_install()} disabled={!!busy}
                onPress={() => void runPluginAction(server, a, 'install', live.length)} />} />;
        })}
      </MachineGroup>}
  </>;
}

function Marketplaces({ server, marketplaces }: { server: Server; marketplaces: PluginMarketplace[] }) {
  const live = useLiveClaude(server);
  const busy = usePlugins((s) => s.busy);
  const [source, setSource] = useState('');
  const [formatError, setFormatError] = useState('');
  const add = () => {
    const value = source.trim();
    if (!SOURCE.test(value)) { setFormatError(m.plugins_marketplace_format_error()); haptic.error(); return; }
    setFormatError('');
    const before = new Set(marketplaces.map((x) => x.name));
    void usePlugins.getState().change(server, 'mkt:add', () => addMarketplaceForServer(server, value),
      (c) => ({ text: m.plugins_action_marketplace_added({ name: c.marketplaces.find((x) => !before.has(x.name))?.name ?? value }) }), live.length)
      .then((ok) => { if (ok) setSource(''); });
  };
  const remove = (mk: PluginMarketplace) => Alert.alert(m.plugins_marketplace_remove_confirm({ name: mk.name }), undefined, [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: m.plugins_remove(), style: 'destructive', onPress: () => void runMarketplaceAction(server, mk.name, 'remove', live.length) },
  ]);
  const autoUpdate = (mk: PluginMarketplace) => void usePlugins.getState().change(server, `mkt:${mk.name}:auto`,
    () => setMarketplaceAutoUpdateForServer(server, mk.name, !mk.auto_update), () => ({ text: null }), live.length);
  return <>
    <PluginsField icon="Plus" value={source} onChangeText={(t) => { setSource(t); setFormatError(''); }}
      placeholder={m.plugins_marketplace_placeholder()} returnKeyType="done" onSubmitEditing={add} />
    <Notice text={formatError} error />
    <MachineGroup>
      <MachineRow title={m.plugins_marketplace_add()} action
        disabled={!!busy || !source.trim()} onPress={add} />
    </MachineGroup>
    {!marketplaces.length ? <InfoNotice text={m.plugins_marketplaces_empty()} /> : marketplaces.map((mk) => {
      const downloaded = mk.last_updated ? m.plugins_marketplace_downloaded({ date: shortDate(mk.last_updated) }) : '';
      const updating = busy === `mkt:${mk.name}:update`;
      return <MachineGroup key={mk.name}>
        <PluginRow title={mk.name} meta={mk.builtin ? m.plugins_marketplace_builtin() : mk.source_label} description={downloaded} />
        {mk.builtin ? null : <>
          <PluginRow title={m.plugins_marketplace_auto_update()}
            meta={mk.auto_update_editable ? undefined : m.plugins_auto_update_unavailable()}
            trailing={<Switch value={mk.auto_update} disabled={!!busy || !mk.auto_update_editable}
              accessibilityLabel={`${m.plugins_marketplace_auto_update()} ${mk.name}`} onValueChange={() => autoUpdate(mk)} />} />
          <MachineRow title={updating ? m.plugins_updating() : m.plugins_update()} action disabled={!!busy}
            onPress={() => void runMarketplaceAction(server, mk.name, 'update', live.length)} />
          <MachineRow title={m.plugins_remove()} danger disabled={!!busy} onPress={() => remove(mk)} />
        </>}
      </MachineGroup>;
    })}
  </>;
}
