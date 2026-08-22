import { useEffect, useState, useCallback } from 'react';
import { Pressable, Text, TextInput, View, ActivityIndicator, ScrollView } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { getRoots, scanDir } from '@hangar/core';
import type { FsRoot, FsEntry } from '@hangar/core';
import * as m from '../../paraglide/messages';

export function CwdPicker({
  onPick,
  selected,
}: {
  onPick: (path: string) => void;
  selected?: string | null;
}) {
  const [roots, setRoots] = useState<FsRoot[]>([]);
  const [rootsLoading, setRootsLoading] = useState(true);
  const [rootsError, setRootsError] = useState(false);
  const [activeRoot, setActiveRoot] = useState<FsRoot | null>(null);
  const [path, setPath] = useState('');
  const [entries, setEntries] = useState<FsEntry[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const [query, setQuery] = useState('');

  const loadRoots = useCallback(async () => {
    try {
      const r = await getRoots();
      setRoots(r);
      if (r.length) {
        setActiveRoot(r[0]);
        setPath(r[0].path);
      }
    } catch {
      setRootsError(true);
    } finally {
      setRootsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadRoots();
  }, [loadRoots]);

  const doScan = useCallback(async (rootPath: string, target: string) => {
    setScanning(true);
    setScanError(null);
    try {
      const res = await scanDir(rootPath, target);
      if (res.error) {
        setScanError(res.error);
        setEntries([]);
      } else {
        setEntries(res.entries);
      }
    } catch (e) {
      setScanError(e instanceof Error ? e.message : 'erro');
      setEntries([]);
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    if (activeRoot) void doScan(activeRoot.path, path || activeRoot.path);
  }, [activeRoot, path, doScan]);

  const selectRoot = (r: FsRoot) => {
    setActiveRoot(r);
    setPath(r.path);
    setQuery('');
  };

  const filtered = (() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((e) => e.name.toLowerCase().includes(q) || e.path.toLowerCase().includes(q));
  })();

  const crumbs = (() => {
    if (!activeRoot) return [] as { label: string; path: string }[];
    const base = activeRoot.path;
    const rest = path.startsWith(base) ? path.slice(base.length) : '';
    const out = [{ label: activeRoot.name, path: base }];
    let acc = base;
    for (const seg of rest.split('/').filter(Boolean)) {
      acc = acc + '/' + seg;
      out.push({ label: seg, path: acc });
    }
    return out;
  })();

  const drilled = !!activeRoot && path !== activeRoot?.path;

  if (rootsLoading) {
    return (
      <View style={styles.center}>
        <ActivityIndicator />
        <Text style={styles.muted}>{m.arquivo_carregando?.() ?? 'Carregando…'}</Text>
      </View>
    );
  }
  if (rootsError) {
    return (
      <View style={styles.center}>
        <Text style={styles.muted}>{m.arquivo_carregar_raizes_erro?.() ?? 'Falha ao carregar raízes'}</Text>
      </View>
    );
  }
  if (!roots.length) {
    return (
      <View style={styles.center}>
        <Text style={styles.muted}>{m.arquivo_sem_raizes?.() ?? 'Nenhuma raiz liberada'}</Text>
      </View>
    );
  }

  return (
    <View style={styles.container}>
      <ScrollView horizontal showsHorizontalScrollIndicator={false} style={styles.chipsScroll} contentContainerStyle={styles.chips}>
        {roots.map((r) => (
          <Pressable
            key={r.path}
            onPress={() => selectRoot(r)}
            style={[styles.chip, activeRoot?.path === r.path && styles.chipOn]}
          >
            <Text style={[styles.chipTxt, activeRoot?.path === r.path && styles.chipTxtOn]}>{r.name}</Text>
          </Pressable>
        ))}
      </ScrollView>

      <TextInput
        style={styles.search}
        value={query}
        onChangeText={setQuery}
        placeholder={m.arquivo_buscar_pasta?.() ?? 'Buscar pasta'}
        placeholderTextColor="#8d8489"
        autoCapitalize="none"
        autoCorrect={false}
      />

      {drilled ? (
        <View style={styles.crumbsWrap}>
          <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.crumbs}>
            {crumbs.map((c, i) => (
              <View key={c.path} style={styles.crumbRow}>
                {i > 0 ? <Text style={styles.sep}>/</Text> : null}
                <Pressable onPress={() => setPath(c.path)} style={styles.crumbBtn}>
                  <Text style={styles.crumbTxt}>{c.label}</Text>
                </Pressable>
              </View>
            ))}
          </ScrollView>
          <Pressable onPress={() => onPick(path)} style={styles.useHere}>
            <Text style={styles.useHereTxt}>{m.arquivo_usar_pasta?.() ?? 'Usar esta pasta'}</Text>
          </Pressable>
        </View>
      ) : null}

      <View style={styles.rows}>
        {scanning ? (
          <View style={styles.center}>
            <ActivityIndicator />
          </View>
        ) : scanError ? (
          <Text style={styles.muted}>{scanError}</Text>
        ) : filtered.length === 0 ? (
          <Text style={styles.muted}>{query.trim() ? (m.arquivo_sem_resultados?.() ?? 'Sem resultados') : (m.arquivo_sem_subpastas?.() ?? 'Sem subpastas')}</Text>
        ) : (
          <ScrollView style={styles.list} contentContainerStyle={{ gap: 4 }}>
            {filtered.map((e) => {
              const sel = selected === e.path;
              return (
                <View key={e.path} style={[styles.row, sel && styles.rowSel]}>
                  <Pressable onPress={() => onPick(e.path)} style={styles.rowBody} accessibilityState={{ selected: sel }}>
                    <Text style={styles.rowName} numberOfLines={1}>
                      {e.name}
                    </Text>
                    <Text style={styles.rowPath} numberOfLines={1}>
                      {e.path}
                    </Text>
                    <View style={styles.badges}>
                      {e.is_git ? <Text style={styles.badgeGit}>git</Text> : null}
                      {e.has_claude_md ? <Text style={styles.badgeCl}>CLAUDE.md</Text> : null}
                    </View>
                  </Pressable>
                  <Pressable onPress={() => setPath(e.path)} style={styles.drill} hitSlop={8} accessibilityLabel={m.arquivo_abrir?.({ nome: e.name }) ?? `Abrir ${e.name}`}>
                    <Text style={styles.drillTxt}>›</Text>
                  </Pressable>
                </View>
              );
            })}
          </ScrollView>
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  container: { flex: 1, gap: theme.base.space[3] },
  center: { padding: theme.base.space[4], alignItems: 'center', gap: 8 },
  muted: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
  chipsScroll: { flexGrow: 0 },
  chips: { flexDirection: 'row', gap: theme.base.space[2], paddingBottom: 2 },
  chip: {
    height: 36,
    paddingHorizontal: theme.base.space[4],
    borderRadius: 9999,
    backgroundColor: theme.tokens.bg.surface,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    justifyContent: 'center',
  },
  chipOn: { backgroundColor: theme.tokens.accent.dim, borderColor: theme.tokens.accent.base },
  chipTxt: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: '500' },
  chipTxtOn: { color: theme.tokens.text.primary },
  search: {
    height: 44,
    backgroundColor: theme.tokens.bg.surface,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    color: theme.tokens.text.primary,
    fontSize: 16,
    paddingHorizontal: theme.base.space[3],
  },
  crumbsWrap: { gap: theme.base.space[2] },
  crumbs: { flexDirection: 'row', alignItems: 'center', gap: 4 },
  crumbRow: { flexDirection: 'row', alignItems: 'center', gap: 2 },
  sep: { color: theme.tokens.text.muted },
  crumbBtn: { paddingHorizontal: 6, paddingVertical: 4, borderRadius: 6 },
  crumbTxt: { color: theme.tokens.accent.base, fontSize: theme.base.text.sm },
  useHere: {
    alignSelf: 'flex-start',
    height: 36,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    justifyContent: 'center',
  },
  useHereTxt: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: '500' },
  rows: { flex: 1, minHeight: 200 },
  list: { flexGrow: 1 },
  row: {
    flexDirection: 'row',
    alignItems: 'stretch',
    borderRadius: theme.base.radius.md,
    gap: theme.base.space[1],
  },
  rowSel: { backgroundColor: theme.tokens.accent.dim, borderWidth: 1, borderColor: theme.tokens.accent.base },
  rowBody: { flex: 1, minHeight: 56, justifyContent: 'center', padding: theme.base.space[2], gap: 2 },
  rowName: { fontSize: theme.base.text.base, fontWeight: '600', color: theme.tokens.text.primary },
  rowPath: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  badges: { flexDirection: 'row', gap: 6, marginTop: 2 },
  badgeGit: { fontSize: 10, fontWeight: '600', color: theme.tokens.accent.base, backgroundColor: theme.tokens.accent.dim, paddingHorizontal: 6, paddingVertical: 2, borderRadius: 9999, overflow: 'hidden' },
  badgeCl: { fontSize: 10, fontWeight: '600', color: theme.tokens.status.warning, backgroundColor: 'rgba(255,159,10,0.14)', paddingHorizontal: 6, paddingVertical: 2, borderRadius: 9999, overflow: 'hidden' },
  drill: { width: 44, justifyContent: 'center', alignItems: 'center' },
  drillTxt: { color: theme.tokens.text.muted, fontSize: 22, fontWeight: '300' },
}));
