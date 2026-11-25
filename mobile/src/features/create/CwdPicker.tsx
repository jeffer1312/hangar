import { useEffect, useMemo, useState, useCallback, useRef } from 'react';
import { FlatList, Pressable, Text, TextInput, View, ScrollView } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getRootsForServer, relativeTime, scanDirForServer } from '@hangar/core';
import type { FsRoot, FsEntry, FsScanError, Server, WorktreeRepo, WorktreeStatus } from '@hangar/core';
import { groupFolders, type FolderRow } from './folderGroups';
import { displayTitle } from '../worktrees/worktreeUi';
import { readProject } from '../../stores/createPreferences';
import { loadErrorText } from './loadError';
import { useCanceledLoadRetry } from './useCanceledLoadRetry';
import { Icon } from '../../ui/Icon';
import { INK } from '../../theme/scale';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

function scanMessage(error: FsScanError): string {
  return ({
    permission_denied: m.arquivo_sem_permissao, unreadable: m.arquivo_ilegivel,
    root_not_allowed: m.arquivo_raiz_nao_liberada, invalid_path: m.arquivo_caminho_invalido,
    not_found: m.arquivo_pasta_nao_encontrada, unknown: m.arquivo_ler_falhou,
  })[error]();
}

// Caminho relativo ao pai da raiz ("projetos/web"), como o rel_path do app de PC.
function relPath(root: string, path: string): string {
  const cut = root.replace(/[/\\]+$/, '').search(/[/\\][^/\\]*$/);
  const parent = cut > 0 ? root.slice(0, cut + 1) : '';
  return parent && path.startsWith(parent) ? path.slice(parent.length) : path;
}

// Linha do menu compacto: nome e, embaixo, quando foi mexida.
const C_ROW = 56;
const WT_ROW = 44;

function parentOf(path: string): string {
  const cut = path.replace(/[/\\]+$/, '').search(/[/\\][^/\\]*$/);
  return cut > 0 ? path.slice(0, cut) : path;
}

type Item =
  | { kind: 'folder'; entry: FsEntry; group?: FolderRow }
  | { kind: 'worktree'; entry: FsEntry; status: WorktreeStatus }
  | { kind: 'merged'; repo: string; count: number };
const itemKey = (it: Item) => (it.kind === 'merged' ? `merged:${it.repo}` : it.entry.path);
const NO_REPOS: WorktreeRepo[] = [];

export function CwdPicker({
  server,
  onPick,
  selected,
  autoSelect = true,
  onDone,
  compact = false,
  repos = NO_REPOS,
}: {
  server: Server;
  // Sem raiz = caminho digitado; não vira o projeto lembrado.
  onPick: (path: string, root?: string) => void;
  selected?: string | null;
  autoSelect?: boolean;
  // Escolha final (linha, "Usar esta pasta", caminho digitado): quem abriu o painel o fecha.
  onDone?: () => void;
  // Painel preso à pílula da tela inicial: menu enxuto que encolhe ao conteúdo.
  compact?: boolean;
  // Worktrees do servidor, para agrupá-las sob o repositório; vazio = lista plana.
  repos?: WorktreeRepo[];
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const [roots, setRoots] = useState<FsRoot[]>([]);
  const [rootsLoading, setRootsLoading] = useState(true);
  const [rootsError, setRootsError] = useState('');
  const [rootsTry, setRootsTry] = useState(0);
  const [activeRoot, setActiveRoot] = useState<FsRoot | null>(null);
  const [path, setPath] = useState('');
  const [entries, setEntries] = useState<FsEntry[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  // Falha de rede tem "tentar de novo"; recusa do backend (permissão, pasta sumida) é só o motivo.
  const [scanFailed, setScanFailed] = useState(false);
  const [scanTry, setScanTry] = useState(0);
  const [query, setQuery] = useState('');
  const searching = !!query.trim();
  const [searchRows, setSearchRows] = useState<{ entry: FsEntry; root: FsRoot }[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchErrors, setSearchErrors] = useState<string[]>([]);
  const [searchTry, setSearchTry] = useState(0);
  const [searchFocused, setSearchFocused] = useState(false);
  const [manualOpen, setManualOpen] = useState(false);
  const [manualPath, setManualPath] = useState('');
  const selectedAtOpen = useRef(selected);
  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(new Set());
  const [openMerged, setOpenMerged] = useState<ReadonlySet<string>>(new Set());
  const initialScanIndex = useRef(0);
  const startRootsRetry = useCanceledLoadRetry(useCallback(() => setRootsTry((n) => n + 1), []));
  const startScanRetry = useCanceledLoadRetry(useCallback(() => setScanTry((n) => n + 1), []));

  useEffect(() => {
    const controller = new AbortController();
    const retry = startRootsRetry();
    setRootsLoading(true);
    setRootsError('');
    setScanError(null);
    void (async () => {
      const r = await getRootsForServer(server, controller.signal);
      if (controller.signal.aborted) return;
      setRoots(r);
      const saved = readProject(server.id);
      if (!autoSelect && r.length) {
        // O painel reabre onde a pasta escolhida está, como o menu do PC, que guarda a pasta aberta.
        const root = r.find((x) => x.path === saved?.root) ?? r[0];
        const current = selectedAtOpen.current;
        setActiveRoot(root);
        setPath(current && current !== root.path && current.startsWith(root.path) ? parentOf(current) : root.path);
        return;
      }
    })().catch((cause: unknown) => {
      if (controller.signal.aborted) return;
      setRootsError(loadErrorText(cause, m.arquivo_carregar_raizes_erro()));
      retry.failed(cause);
    }).finally(() => {
      if (!controller.signal.aborted) setRootsLoading(false);
    });
    return () => { retry.cancel(); controller.abort(); };
  }, [server, autoSelect, rootsTry, startRootsRetry]);

  useEffect(() => { initialScanIndex.current = 0; }, [server, roots, autoSelect]);

  useEffect(() => {
    if (rootsLoading || rootsError || !roots.length || !autoSelect) return;
    const controller = new AbortController();
    const retry = startScanRetry();
    setScanError(null);
    setScanFailed(false);
    void (async () => {
      const saved = readProject(server.id);
      if (saved && !roots.some((root) => root.path === saved.root)) {
        setScanError(m.criar_projeto_indisponivel());
        return;
      }
      const candidates = saved ? roots.filter((root) => root.path === saved.root) : roots;
      for (const root of candidates.slice(initialScanIndex.current)) {
        const cwd = saved?.cwd ?? root.path;
        const result = await scanDirForServer(server, root.path, cwd, controller.signal);
        if (controller.signal.aborted) return;
        if (result.error) {
          initialScanIndex.current++;
          setScanError(saved ? `${m.criar_projeto_indisponivel()} ${scanMessage(result.error)}` : scanMessage(result.error));
          continue;
        }
        onPick(cwd, root.path);
        return;
      }
    })().catch((cause: unknown) => {
      if (controller.signal.aborted) return;
      setScanError(loadErrorText(cause, m.arquivo_ler_falhou()));
      setScanFailed(true);
      retry.failed(cause);
    });
    return () => { retry.cancel(); controller.abort(); };
  }, [server, onPick, autoSelect, roots, rootsLoading, rootsError, scanTry, startScanRetry]);

  useEffect(() => {
    if (rootsLoading || !activeRoot) return;
    const controller = new AbortController();
    const retry = startScanRetry();
    setScanning(true);
    setScanError(null);
    setScanFailed(false);
    void scanDirForServer(server, activeRoot.path, path || activeRoot.path, controller.signal)
      .then((res) => {
        if (controller.signal.aborted) return;
        setEntries(res.error ? [] : res.entries);
        if (res.error) setScanError(scanMessage(res.error));
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        setScanError(loadErrorText(e, m.arquivo_ler_falhou()));
        setScanFailed(true);
        setEntries([]);
        retry.failed(e);
      })
      .finally(() => { if (!controller.signal.aborted) setScanning(false); });
    return () => { retry.cancel(); controller.abort(); };
  }, [activeRoot, path, server, rootsLoading, scanTry, startScanRetry]);

  // A busca lê cada raiz uma vez por tentativa, sem reiniciar os pedidos a cada letra.
  useEffect(() => {
    if (!searching || rootsLoading || !roots.length) return;
    const controller = new AbortController();
    setSearchRows([]);
    setSearchErrors([]);
    setSearchLoading(true);
    void Promise.all(roots.map(async (root) => {
      try {
        const result = await scanDirForServer(server, root.path, root.path, controller.signal);
        if (controller.signal.aborted) return;
        if (result.error) throw new Error(scanMessage(result.error));
        setSearchRows((current) => [...current, ...result.entries.map((entry) => ({ entry, root }))]);
      } catch (cause) {
        if (!controller.signal.aborted) setSearchErrors((current) => [...current, `${root.name}: ${loadErrorText(cause, m.arquivo_ler_falhou())}`]);
      }
    })).finally(() => { if (!controller.signal.aborted) setSearchLoading(false); });
    return () => controller.abort();
  }, [searching, rootsLoading, roots, server, searchTry]);

  // Raiz escolhida já é a pasta, como no menu do PC; o painel continua aberto para descer nela.
  const selectRoot = (r: FsRoot) => {
    setActiveRoot(r);
    setPath(r.path);
    onPick(r.path, r.path);
  };
  const drill = (p: string, root = activeRoot) => {
    setActiveRoot(root);
    setPath(p);
    setQuery('');
  };
  const choose = (p: string, root?: string) => {
    onPick(p, root);
    onDone?.();
  };
  const useTyped = () => {
    const p = manualPath.trim();
    if (p) choose(p);
  };

  const q = query.trim().toLowerCase();
  const rootsByPath = new Map(searchRows.map(({ entry, root }) => [entry.path, root]));
  const rootFor = (entry: FsEntry) => searching ? rootsByPath.get(entry.path) ?? activeRoot! : activeRoot!;
  const filtered = !q ? entries : [...new Map(searchRows
    .filter(({ entry, root }) => entry.name.toLowerCase().includes(q) || relPath(root.path, entry.path).toLowerCase().includes(q))
    .map(({ entry }) => [entry.path, entry])).values()];
  const listLoading = searching ? searchLoading && !filtered.length : scanning;
  const listError = searching ? null : scanError;
  // Buscando, a lista é plana: o nome da worktree precisa aparecer mesmo com o grupo fechado.
  const items = useMemo((): Item[] => q ? filtered.map((entry) => ({ kind: 'folder', entry })) : groupFolders(filtered, repos).flatMap((row) => {
    const sized = row.visible.length + row.merged.length > 0;
    const out: Item[] = [{ kind: 'folder', entry: row.entry, group: sized ? row : undefined }];
    if (!sized || !openGroups.has(row.entry.path)) return out;
    out.push(...row.visible.map((w): Item => ({ kind: 'worktree', entry: w.entry, status: w.status })));
    if (openMerged.has(row.entry.path)) out.push(...row.merged.map((w): Item => ({ kind: 'worktree', entry: w.entry, status: w.status })));
    else if (row.merged.length) out.push({ kind: 'merged', repo: row.entry.path, count: row.merged.length });
    return out;
  }), [q, filtered, repos, openGroups, openMerged]);
  const toggle = (set: (fn: (cur: ReadonlySet<string>) => ReadonlySet<string>) => void, key: string) => set((cur) => {
    const next = new Set(cur);
    if (!next.delete(key)) next.add(key);
    return next;
  });
  const groupButton = (row: FolderRow) => {
    const open = openGroups.has(row.entry.path);
    return (
      <Pressable onPress={() => toggle(setOpenGroups, row.entry.path)} hitSlop={6} accessibilityRole="button"
        accessibilityLabel={`${row.entry.name}: ${m.native_create_worktrees({ n: row.visible.length + row.merged.length })}`} accessibilityState={{ expanded: open }}
        style={({ pressed }) => [styles.group, pressed && { backgroundColor: c.hover }]}>
        <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={12} color={c.faint} />
        <Text style={[styles.groupTxt, { color: c.muted }]}>{row.visible.length + row.merged.length}</Text>
        <Icon name="GitBranch" size={12} color={c.faint} />
      </Pressable>
    );
  };
  // No menu compacto o grupo de worktrees é uma etiqueta "N cópias", com o mesmo toque que abre a lista delas.
  const copiesBadge = (row: FolderRow) => {
    const open = openGroups.has(row.entry.path);
    const n = row.visible.length + row.merged.length;
    return (
      <Pressable onPress={() => toggle(setOpenGroups, row.entry.path)} hitSlop={{ top: 10, bottom: 10, left: 8, right: 8 }} accessibilityRole="button"
        accessibilityLabel={`${row.entry.name}: ${m.native_create_worktrees({ n })}`} accessibilityState={{ expanded: open }}
        style={({ pressed }) => [styles.cBadge, { backgroundColor: open || pressed ? c.accentDim : c.hover }]}>
        <Text style={[styles.cBadgeTxt, { color: c.accent }]} numberOfLines={1}>
          {n === 1 ? m.native_create_copies_1() : m.native_create_copies({ n })}
        </Text>
      </Pressable>
    );
  };
  const worktreeRow = (e: FsEntry, status: WorktreeStatus) => {
    const on = selected === e.path;
    const title = displayTitle(status);
    return (
      <Pressable onPress={() => choose(e.path, activeRoot?.path)} accessibilityRole="button" accessibilityLabel={title}
        accessibilityState={{ selected: on }}
        style={({ pressed }) => [styles.wtRow, on && { backgroundColor: c.accentDim }, pressed && !on && { backgroundColor: c.hover }]}>
        <Text style={[styles.wtName, { color: c.text }]} numberOfLines={1}>{title}</Text>
        <Text style={[styles.wtBranch, { color: c.faint }]} numberOfLines={1}>{status.branch ?? ''}</Text>
        {status.sessions.length ? <Text style={[styles.wtChip, { color: c.accent }]}>{m.native_create_worktree_session()}</Text> : null}
      </Pressable>
    );
  };
  // Linhas que não são pasta comum, iguais nas duas listas.
  const special = (item: Item) => item.kind === 'worktree' ? worktreeRow(item.entry, item.status)
    : item.kind === 'merged' ? mergedRow(item.repo, item.count) : null;
  const mergedRow = (repo: string, count: number) => (
    <Pressable onPress={() => toggle(setOpenMerged, repo)} accessibilityRole="button"
      accessibilityLabel={m.native_create_worktrees_merged({ n: count })} style={styles.wtRow}>
      <Text style={[styles.wtMore, { color: c.faint }]}>{m.native_create_worktrees_merged({ n: count })}</Text>
    </Pressable>
  );

  const crumbs = (() => {
    if (!activeRoot) return [] as { label: string; path: string }[];
    const base = activeRoot.path;
    const rest = path.startsWith(base) ? path.slice(base.length) : '';
    const out = [{ label: activeRoot.name, path: base }];
    for (const match of rest.matchAll(/[^/\\]+/g)) {
      out.push({ label: match[0], path: path.slice(0, base.length + match.index + match[0].length) });
    }
    return out;
  })();

  if (compact) {
    const textRetry = (onPress: () => void) => (
      <Pressable onPress={onPress} accessibilityRole="button" hitSlop={6} style={styles.cTextBtn}>
        <Text style={[styles.cTextBtnTxt, { color: c.accent }]}>{m.native_retry()}</Text>
      </Pressable>
    );
    const note = (text: string, alert = false) => (
      <Text style={[styles.cNote, { color: alert ? theme.tokens.status.error : c.faint }]}
            accessibilityRole={alert ? 'alert' : undefined}>{text}</Text>
    );
    const inSub = !searching && !!activeRoot && path !== '' && path !== activeRoot.path;
    const manualValid = !!manualPath.trim();

    const cRows = !activeRoot ? (
      scanError ? <View style={styles.cState}>
        {note(scanError, true)}
        {scanFailed ? textRetry(() => setScanTry((n) => n + 1)) : null}
      </View> : null
    ) : listLoading ? (
      <View style={styles.cSkeletons}>
        {[120, 168, 96].map((w) => <View key={w} style={[styles.cSkeleton, { width: w, backgroundColor: c.border }]} />)}
      </View>
    ) : listError ? (
      <View style={styles.cState}>
        {note(listError, scanFailed)}
        {scanFailed ? textRetry(() => setScanTry((n) => n + 1)) : null}
      </View>
    ) : filtered.length === 0 && (!searching || !searchErrors.length) ? (
      note(q ? m.native_create_no_results() : m.native_create_no_subfolders())
    ) : (
      <FlatList
        data={items}
        keyExtractor={itemKey}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        // Altura explícita: dentro do painel que encolhe, a lista sem altura colapsava em uma linha.
        style={[styles.cList, { height: items.slice(0, 8).reduce((h, it) => h + (it.kind === 'folder' ? C_ROW : WT_ROW), 0) }]}
        ItemSeparatorComponent={() => <View style={[styles.cSep, { backgroundColor: c.border }]} />}
        renderItem={({ item }) => {
          if (item.kind !== 'folder') return special(item);
          const e = item.entry;
          const on = selected === e.path;
          const when = relativeTime(e.mtime);
          return (
            <View style={[styles.cRowWrap, on && { backgroundColor: c.accentDim }]}>
              <Pressable
                onPress={() => choose(e.path, rootFor(e).path)}
                accessibilityRole="button"
                accessibilityLabel={e.name}
                accessibilityState={{ selected: on }}
                style={({ pressed }) => [styles.cRow, pressed && !on && { backgroundColor: c.hover }]}
              >
                <Icon name="Folder" size={18} color={c.accent} />
                <View style={styles.cText}>
                  <Text style={[styles.cName, { color: c.text }]} numberOfLines={1} ellipsizeMode="tail">{e.name}</Text>
                  {searching ? <Text style={[styles.cPath, { color: c.muted }]} numberOfLines={1}>{rootFor(e).name} · {relPath(rootFor(e).path, e.path)}</Text>
                    : e.is_git || when ? (
                      <View style={styles.cMeta}>
                        {e.is_git ? <Icon name="GitBranch" size={11} color={c.faint} /> : null}
                        {when ? <Text style={[styles.cWhen, { color: c.faint }]} numberOfLines={1}>{when}</Text> : null}
                      </View>
                    ) : null}
                </View>
              </Pressable>
              {item.group ? copiesBadge(item.group) : null}
              <Pressable onPress={() => drill(e.path, rootFor(e))} style={({ pressed }) => [styles.cDrill, pressed && { backgroundColor: c.hover }]}
                         accessibilityRole="button" accessibilityLabel={m.native_create_open({ nome: e.name })}>
                <Icon name="ChevronRight" size={14} color={c.faint} />
              </Pressable>
            </View>
          );
        }}
      />
    );

    return (
      <View style={styles.cContainer}>
        {rootsLoading ? (
          <View style={styles.cTabsRow}>{[64, 88].map((w) => <View key={w} style={[styles.cTabSkeleton, { width: w, backgroundColor: c.border }]} />)}</View>
        ) : rootsError ? (
          <View style={styles.cState}>
            {note(`${m.native_create_roots_failed()} ${rootsError}`, true)}
            {textRetry(() => setRootsTry((n) => n + 1))}
          </View>
        ) : !roots.length ? (
          note(m.arquivo_sem_raizes())
        ) : (
          <ScrollView horizontal showsHorizontalScrollIndicator={false} style={styles.chipsScroll} contentContainerStyle={styles.cChips}
                      accessibilityLabel={m.native_create_roots()} keyboardShouldPersistTaps="handled">
            {roots.map((r) => {
              const on = activeRoot?.path === r.path;
              return (
                <Pressable key={r.path} onPress={() => selectRoot(r)} accessibilityRole="button" accessibilityState={{ selected: on }}
                           hitSlop={{ top: 6, bottom: 6 }} style={[styles.chip, on && styles.chipOn]}>
                  <Text style={[styles.chipTxt, on && styles.chipTxtOn]} numberOfLines={1}>{r.name}</Text>
                </Pressable>
              );
            })}
          </ScrollView>
        )}

        {activeRoot ? (
          <View style={[styles.cSearch, { backgroundColor: c.inset }]}>
            <Icon name="Search" size={14} color={c.faint} />
            <TextInput
              style={[styles.searchInput, { color: c.text }]}
              value={query}
              onChangeText={setQuery}
              placeholder={m.native_create_search()}
              accessibilityLabel={m.native_create_search()}
              placeholderTextColor={c.faint}
              autoCapitalize="none"
              autoCorrect={false}
              returnKeyType="search"
              clearButtonMode="while-editing"
            />
          </View>
        ) : null}

        {inSub ? (
          <View style={styles.cPathRow}>
            <Pressable onPress={() => drill(parentOf(path))} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.comum_voltar()}
                       style={({ pressed }) => [styles.cBack, pressed && { backgroundColor: c.hover }]}>
              <Icon name="ChevronLeft" size={14} color={c.muted} />
            </Pressable>
            <Text style={[styles.cPath, { color: c.muted }]} numberOfLines={1} ellipsizeMode="head"
                  accessibilityLabel={`${m.native_create_path()}: ${relPath(activeRoot.path, path)}`}>
              {relPath(activeRoot.path, path)}
            </Text>
          </View>
        ) : null}

        <View style={styles.cRows}>{cRows}</View>
        {searching && searchErrors.length ? <View style={styles.cState}>
          {note(searchErrors.join('\n'), true)}
          {textRetry(() => setSearchTry((n) => n + 1))}
        </View> : null}
        {searching && searchLoading && filtered.length ? note(m.comum_carregando()) : null}

        <View style={[styles.cFooter, { borderTopColor: c.border }]}>
          <Pressable onPress={() => setManualOpen((v) => !v)} accessibilityRole="button" accessibilityState={{ expanded: manualOpen }}
                     style={({ pressed }) => [styles.cButton, styles.cButtonOutline, { borderColor: c.border }, pressed && { backgroundColor: c.hover }]}>
            <Text style={[styles.cButtonTxt, { color: c.muted }]} numberOfLines={1}>{m.native_create_computer_folder()}</Text>
          </Pressable>
          {activeRoot ? (
            <Pressable onPress={() => choose(path || activeRoot.path, activeRoot.path)} accessibilityRole="button"
                       style={({ pressed }) => [styles.cButton, { backgroundColor: c.accent }, pressed && styles.cPressed]}>
              {/* O nome da pasta no botão diz o que o toque escolhe: a raiz ou a subpasta aberta. */}
              <Text style={[styles.cButtonTxt, styles.cPrimaryTxt]} numberOfLines={1} ellipsizeMode="middle">
                {m.native_create_use_named({ nome: !path || path === activeRoot.path ? activeRoot.name
                  : path.split(/[/\\]/).filter(Boolean).pop() ?? activeRoot.name })}
              </Text>
            </Pressable>
          ) : null}
        </View>
        {manualOpen ? (
          <View style={styles.manual}>
            <TextInput
              style={[styles.cSearch, styles.cManualInput, { backgroundColor: c.inset, color: c.text }]}
              value={manualPath}
              onChangeText={setManualPath}
              onSubmitEditing={useTyped}
              placeholder={m.native_create_path_placeholder()}
              accessibilityLabel={m.native_create_path_aria()}
              placeholderTextColor={c.faint}
              autoCapitalize="none"
              autoCorrect={false}
              autoFocus
              returnKeyType="go"
            />
            <Pressable onPress={useTyped} disabled={!manualValid} accessibilityRole="button" accessibilityState={{ disabled: !manualValid }}
                       style={[styles.cPill, { borderColor: c.accent }, !manualValid && styles.disabled]}>
              <Text style={[styles.cPillTxt, { color: c.accent }]}>{m.native_create_use()}</Text>
            </Pressable>
          </View>
        ) : null}
      </View>
    );
  }

  const retry = (onPress: () => void) => (
    <Pressable onPress={onPress} accessibilityRole="button" style={styles.outline}>
      <Text style={styles.outlineTxt}>{m.native_retry()}</Text>
    </Pressable>
  );

  // Esqueleto da trilha de raízes: duas pílulas de 96 × 32.
  const rootsBlock = rootsLoading ? (
    <View style={styles.chips}>{[0, 1].map((i) => <View key={i} style={[styles.skeleton, styles.chipSkeleton]} />)}</View>
  ) : rootsError ? (
    <View style={styles.state}>
      <Text style={styles.error} accessibilityRole="alert">{`${m.native_create_roots_failed()} ${rootsError}`}</Text>
      {retry(() => setRootsTry((n) => n + 1))}
    </View>
  ) : !roots.length ? (
    <Text style={styles.muted}>{m.arquivo_sem_raizes()}</Text>
  ) : (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} style={styles.chipsScroll} contentContainerStyle={styles.chips}
                accessibilityLabel={m.native_create_roots()} keyboardShouldPersistTaps="handled">
      {roots.map((r) => {
        const on = activeRoot?.path === r.path;
        return (
          <Pressable key={r.path} onPress={() => selectRoot(r)} accessibilityRole="button" accessibilityState={{ selected: on }}
                     hitSlop={{ top: 6, bottom: 6 }} style={[styles.chip, on && styles.chipOn]}>
            <Text style={[styles.chipTxt, on && styles.chipTxtOn]} numberOfLines={1}>{r.name}</Text>
          </Pressable>
        );
      })}
    </ScrollView>
  );

  const rows = !activeRoot ? (
    scanError ? <View style={styles.state}>
      <Text style={styles.muted} accessibilityRole="alert">{scanError}</Text>
      {scanFailed ? retry(() => setScanTry((n) => n + 1)) : null}
    </View> : null
  ) : listLoading ? (
    // Cinco linhas de esqueleto: nome e detalhe, como o render_rows do PC.
    <View style={styles.skeletonList}>
      {[0, 1, 2, 3, 4].map((i) => (
        <View key={i} style={styles.skeletonRow}>
          <View style={[styles.skeleton, { width: 160, height: 12 }]} />
          <View style={[styles.skeleton, styles.skeletonSoft, { width: 240, height: 10 }]} />
        </View>
      ))}
    </View>
  ) : listError ? (
    <View style={styles.state}>
      <Text style={scanFailed ? styles.error : styles.muted} accessibilityRole="alert">{listError}</Text>
      {scanFailed ? retry(() => setScanTry((n) => n + 1)) : null}
    </View>
  ) : filtered.length === 0 && (!searching || !searchErrors.length) ? (
    <Text style={styles.muted}>{q ? m.native_create_no_results() : m.native_create_no_subfolders()}</Text>
  ) : (
    <FlatList
      data={items}
      keyExtractor={itemKey}
      keyboardShouldPersistTaps="handled"
      keyboardDismissMode="on-drag"
      style={styles.list}
      renderItem={({ item }) => {
        if (item.kind !== 'folder') return special(item);
        const e = item.entry;
        const on = selected === e.path;
        const when = relativeTime(e.mtime);
        return (
          <View style={styles.rowWrap}>
            <Pressable
              onPress={() => choose(e.path, rootFor(e).path)}
              accessibilityRole="button"
              accessibilityLabel={e.name}
              accessibilityState={{ selected: on }}
              style={({ pressed }) => [styles.row, on && styles.rowOn, pressed && !on && styles.rowPressed]}
            >
              <Text style={styles.rowName} numberOfLines={1}>{e.name}</Text>
              <View style={styles.meta}>
                <Text style={styles.rowPath} numberOfLines={1}>{searching ? `${rootFor(e).name} · ` : ''}{relPath(rootFor(e).path, e.path)}</Text>
                {e.is_git ? <Text style={styles.badge}>git</Text> : null}
                {e.has_claude_md ? <Text style={styles.badge}>CLAUDE.md</Text> : null}
                {when ? <Text style={styles.when}>{when}</Text> : null}
              </View>
            </Pressable>
            {item.group ? groupButton(item.group) : null}
            <Pressable onPress={() => drill(e.path, rootFor(e))} style={({ pressed }) => [styles.drill, pressed && styles.rowPressed]}
                       accessibilityRole="button" accessibilityLabel={m.native_create_open({ nome: e.name })}>
              <Icon name="ChevronRight" size={16} color={theme.tokens.text.secondary} />
            </Pressable>
          </View>
        );
      }}
    />
  );

  return (
    <View style={styles.container}>
      {rootsBlock}

      {activeRoot ? (
        <>
          <View style={[styles.search, searchFocused && styles.searchOn]}>
            <Icon name="Search" size={14} color={theme.tokens.text.muted} />
            <TextInput
              style={styles.searchInput}
              value={query}
              onChangeText={setQuery}
              onFocus={() => setSearchFocused(true)}
              onBlur={() => setSearchFocused(false)}
              placeholder={m.native_create_search()}
              accessibilityLabel={m.native_create_search()}
              placeholderTextColor={theme.tokens.text.muted}
              autoCapitalize="none"
              autoCorrect={false}
              returnKeyType="search"
              clearButtonMode="while-editing"
            />
          </View>

          <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.crumbs}
                      accessibilityLabel={m.native_create_path()} keyboardShouldPersistTaps="handled" style={styles.chipsScroll}>
            {crumbs.map((c) => (
              <Pressable key={c.path} onPress={() => drill(c.path)} style={({ pressed }) => [styles.crumb, pressed && styles.rowPressed]}
                         accessibilityRole="button">
                <Text style={styles.crumbTxt} numberOfLines={1}>{c.label}</Text>
              </Pressable>
            ))}
          </ScrollView>
        </>
      ) : null}

      <View style={styles.rows}>{rows}</View>
      {searching && searchErrors.length ? <View style={styles.state}>
        <Text style={styles.error} accessibilityRole="alert">{searchErrors.join('\n')}</Text>
        {retry(() => setSearchTry((n) => n + 1))}
      </View> : null}
      {searching && searchLoading && filtered.length ? <Text style={styles.muted}>{m.comum_carregando()}</Text> : null}

      <View style={styles.footer}>
        {activeRoot ? (
          <Pressable onPress={() => choose(path || activeRoot.path, activeRoot.path)} accessibilityRole="button" style={styles.outline}>
            <Text style={styles.outlineTxt}>{m.native_create_use_folder()}</Text>
          </Pressable>
        ) : null}
        {/* No celular não há seletor de pastas do sistema para o servidor: "Abrir pasta…" é digitar o caminho. */}
        <Pressable onPress={() => setManualOpen((v) => !v)} accessibilityRole="button" accessibilityState={{ expanded: manualOpen }}
                   style={({ pressed }) => [styles.ghost, pressed && styles.rowPressed]}>
          <Icon name="FolderOpen" size={14} color={theme.tokens.text.secondary} />
          <Text style={styles.ghostTxt}>{m.native_create_computer_folder()}</Text>
        </Pressable>
      </View>
      {manualOpen ? (
        <View style={styles.manual}>
          <TextInput
            style={styles.manualInput}
            value={manualPath}
            onChangeText={setManualPath}
            onSubmitEditing={useTyped}
            placeholder={m.native_create_path_placeholder()}
            accessibilityLabel={m.native_create_path_aria()}
            placeholderTextColor={theme.tokens.text.muted}
            autoCapitalize="none"
            autoCorrect={false}
            autoFocus
            returnKeyType="go"
          />
          <Pressable onPress={useTyped} disabled={!manualPath.trim()} accessibilityRole="button"
                     accessibilityState={{ disabled: !manualPath.trim() }} style={[styles.outline, !manualPath.trim() && styles.disabled]}>
            <Text style={styles.outlineTxt}>{m.native_create_use()}</Text>
          </Pressable>
        </View>
      ) : null}
    </View>
  );
}

// Medidas do render_compact_folders do PC (create.rs:1602): gap 12, pílulas de raiz, campo pequeno
// com lupa, linhas com borda raio 8 e a seta fora delas. Alturas sobem para o toque.
const styles = StyleSheet.create((theme) => ({
  container: { flex: 1, gap: theme.base.space[3] },
  state: { gap: theme.base.space[2], alignItems: 'flex-start', paddingVertical: theme.base.space[2] },
  muted: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, paddingVertical: theme.base.space[2] },
  error: { fontSize: theme.base.text.xs, color: theme.tokens.status.error },
  chipsScroll: { flexGrow: 0, flexShrink: 0 },
  chips: { flexDirection: 'row', gap: theme.base.space[2] },
  chip: {
    height: 32,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    justifyContent: 'center',
  },
  chipOn: { backgroundColor: theme.tokens.accent.dim, borderColor: theme.tokens.accent.base },
  chipTxt: { fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  chipTxtOn: { fontWeight: theme.base.weight.medium },
  skeleton: { borderRadius: theme.base.radius.sm, backgroundColor: theme.tokens.border.default },
  skeletonSoft: { opacity: 0.6 },
  chipSkeleton: { width: 96, height: 32, borderRadius: theme.base.radius.full },
  skeletonList: { gap: theme.base.space[1.5] },
  skeletonRow: { gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[2] },
  search: {
    height: 40,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
  },
  searchOn: { borderColor: theme.tokens.accent.base },
  // 16 no campo: abaixo disso o iOS dá zoom ao focar.
  searchInput: { flex: 1, height: '100%', color: theme.tokens.text.primary, fontSize: theme.base.text.sm, paddingVertical: 0 },
  crumbs: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  crumb: { minHeight: 32, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, justifyContent: 'center' },
  crumbTxt: { fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  rows: { flex: 1, minHeight: 96 },
  list: { flex: 1 },
  rowWrap: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], paddingBottom: theme.base.space[0.5] },
  row: {
    flex: 1,
    minWidth: 0,
    minHeight: 48,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1.5],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    justifyContent: 'center',
    gap: theme.base.space[0.5],
  },
  rowOn: { backgroundColor: theme.tokens.accent.dim, borderColor: theme.tokens.accent.base },
  rowPressed: { backgroundColor: theme.tokens.border.subtle },
  rowName: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  meta: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5] },
  rowPath: { flex: 1, minWidth: 0, fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  badge: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xxxs,
    color: theme.tokens.text.muted,
    paddingHorizontal: theme.base.space[1.5],
    borderRadius: theme.base.radius.xs,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    overflow: 'hidden',
  },
  when: { flexShrink: 0, fontSize: theme.base.text.xxs, color: theme.tokens.text.muted, opacity: 0.8 },
  drill: { width: 40, height: 44, borderRadius: theme.base.radius.sm, alignItems: 'center', justifyContent: 'center' },
  footer: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  outline: {
    minHeight: 36,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    justifyContent: 'center',
  },
  outlineTxt: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium, color: theme.tokens.text.primary },
  ghost: { minHeight: 36, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5] },
  ghostTxt: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  manual: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  manualInput: {
    flex: 1,
    height: 40,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    color: theme.tokens.text.primary,
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.sm,
  },
  disabled: { opacity: 0.5 },
  // Modo compacto: menu do iPhone com as cores do Hangar; sem flex:1 para o painel encolher ao conteúdo.
  cContainer: { flexShrink: 1, gap: theme.base.space[2] },
  cState: { gap: theme.base.space[1], alignItems: 'flex-start', paddingVertical: theme.base.space[1] },
  cNote: { fontSize: theme.base.text.xs, paddingVertical: theme.base.space[1.5] },
  cTextBtn: { height: 32, justifyContent: 'center', paddingHorizontal: theme.base.space[1] },
  cTextBtnTxt: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  cTabsRow: { flexDirection: 'row', gap: theme.base.space[1.5] },
  cTabSkeleton: { height: 28, borderRadius: theme.base.radius.sm },
  cSearch: { height: 36, borderRadius: theme.base.radius.sm, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[2] },
  cPathRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], marginTop: -2 },
  cBack: { width: 24, height: 24, borderRadius: theme.base.radius.sm, alignItems: 'center', justifyContent: 'center' },
  cPath: { flex: 1, minWidth: 0, fontSize: theme.base.text.xs },
  cRows: { flexShrink: 1 },
  cList: { flexGrow: 0, flexShrink: 1 },
  cSkeletons: { gap: theme.base.space[4], paddingVertical: theme.base.space[3], paddingLeft: theme.base.space[2] },
  cSkeleton: { height: 8, borderRadius: theme.base.radius.full },
  cRowWrap: { flexDirection: 'row', alignItems: 'center', borderRadius: theme.base.radius.md },
  cRow: { flex: 1, minWidth: 0, height: C_ROW, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingLeft: theme.base.space[2], borderRadius: theme.base.radius.md },
  cText: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  cMeta: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  cName: { flexShrink: 1, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  cWhen: { flexShrink: 1, fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] },
  cBadge: { height: 24, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.full, justifyContent: 'center', marginLeft: theme.base.space[1] },
  cBadgeTxt: { fontSize: theme.base.text.xxxs, fontWeight: theme.base.weight.semibold },
  cDrill: { width: 44, height: C_ROW, borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center' },
  // Recuo até o texto: 8 de margem + ícone 18 + gap 12.
  cSep: { height: StyleSheet.hairlineWidth, marginLeft: theme.base.space[10] - theme.base.space[0.5] },
  cChips: { flexDirection: 'row', gap: theme.base.space[1.5] },
  cFooter: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingTop: theme.base.space[2], borderTopWidth: StyleSheet.hairlineWidth },
  cButton: { flex: 1, minWidth: 0, height: 44, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center' },
  cButtonOutline: { borderWidth: 1 },
  cButtonTxt: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  cPrimaryTxt: { color: INK.onAccent },
  cPill: { height: 32, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.full, borderWidth: 1, justifyContent: 'center' },
  cPillTxt: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  cPressed: { opacity: 0.85 },
  cManualInput: { flex: 1, fontFamily: theme.base.fontMono, fontSize: theme.base.text.sm },
  group: { minWidth: 44, height: 44, borderRadius: theme.base.radius.sm, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: theme.base.space[0.5], paddingHorizontal: theme.base.space[1] },
  groupTxt: { fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  // Recuo de um nível: a worktree mora dentro do repositório da linha de cima.
  wtRow: { minHeight: WT_ROW, marginLeft: theme.base.space[6] + theme.base.space[0.5], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm },
  wtName: { flexShrink: 1, fontSize: theme.base.text.sm },
  wtBranch: { flex: 1, minWidth: 0, fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
  wtChip: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold },
  wtMore: { fontSize: theme.base.text.xs },
}));
