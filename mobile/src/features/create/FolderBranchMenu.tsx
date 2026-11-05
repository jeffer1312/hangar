import { useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  basename, deleteWorktreeForServer, folderGitBranchForServer, folderGitSwitchForServer,
  type FolderBranches, type FolderGit, type Server,
} from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { superficie } from '../../theme/superficie';
import { TextTabs } from '../usage/HomeUsageCard';
import * as m from '../../paraglide/messages';

type Props = {
  server: Server; cwd: string; info: FolderBranches; git: FolderGit;
  onBack: () => void;
  // O estado relido e o aviso do que mudou.
  onDone: (status: FolderGit, note: string) => void;
  // A recusa pode vir de um estado mais novo que o da tela.
  onRefused: () => void;
  onOpenFolder: (path: string) => void;
};
type Tab = 'switch' | 'create';
type Pending = { kind: Tab; branch: string };
type Elsewhere = { branch: string; path: string; main: boolean };

const SESSIONS_REFUSAL = 'erro_git_folder_sessions';
const reason = (e: unknown) => (e instanceof Error ? e.message.replace(/^\d{3}: /, '') : String(e));

function closeRefusal(e: unknown, folder: string): string {
  const text = reason(e);
  const busy = text.match(/sessão aberta dentro: (.+)$/);
  if (busy) return m.native_git_elsewhere_busy({ pasta: folder, nomes: busy[1] });
  if (text.includes('há arquivos que serão perdidos')) return m.native_git_elsewhere_dirty({ pasta: folder });
  return m.native_git_elsewhere_failed({ pasta: folder, motivo: text });
}

export function FolderBranchMenu({ server, cwd, info, git, onBack, onDone, onRefused, onOpenFolder }: Props) {
  const { theme } = useUnistyles();
  const [tab, setTab] = useState<Tab>('switch');
  const [query, setQuery] = useState('');
  const [name, setName] = useState('');
  const [base, setBase] = useState('');
  const [checkout, setCheckout] = useState(true);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState('');
  const [pending, setPending] = useState<Pending | null>(null);
  const [elsewhere, setElsewhere] = useState<Elsewhere | null>(null);

  const dirty = (git.dirty ?? 0) > 0;
  const sessions = git.sessions ?? [];
  const current = git.current ?? '';
  const typed = name.trim();
  const clear = () => { setFailure(''); setPending(null); setElsewhere(null); };

  const run = async (action: Pending, confirm: boolean) => {
    setBusy(true);
    clear();
    try {
      const status = action.kind === 'switch'
        ? await folderGitSwitchForServer(server, cwd, action.branch, confirm)
        : await folderGitBranchForServer(server, cwd, action.branch, { checkout, base: base || undefined, confirmSessions: confirm });
      haptic.success();
      onDone(status, action.kind === 'switch' ? m.native_folder_git_switched({ branch: status.current ?? action.branch }) : m.native_folder_git_created({ branch: action.branch }));
    } catch (e) {
      if ((e as { code?: string } | null)?.code === SESSIONS_REFUSAL) setPending(action);
      else { setFailure(reason(e)); haptic.error(); }
      onRefused();
      setBusy(false);
    }
  };

  const request = (action: Pending) => {
    clear();
    if (sessions.length && (action.kind === 'switch' || checkout)) setPending(action);
    else void run(action, false);
  };

  const pickBranch = (branch: string) => {
    if (branch === current) return;
    const held = info.elsewhere?.[branch];
    if (held) { clear(); setElsewhere({ branch, path: held.path, main: !!held.main }); haptic.selection(); return; }
    request({ kind: 'switch', branch });
  };

  // Fecha a outra worktree (o servidor recusa com sessão ou alteração) e então traz a branch para esta pasta.
  const closeElsewhere = async ({ branch, path }: Elsewhere) => {
    setBusy(true);
    setFailure('');
    try {
      await deleteWorktreeForServer(server, { repo: path, path, confirm: false, delete_branch: false, keep_branch: true });
    } catch (e) {
      setFailure(closeRefusal(e, basename(path)));
      haptic.error();
      setBusy(false);
      return;
    }
    await run({ kind: 'switch', branch }, false);
  };

  const search = query.trim().toLowerCase();
  const remoteLabel = m.native_folder_git_remote();
  const rows = [...info.branches.map((b) => ({ b, remote: false })), ...info.remotes.map((b) => ({ b, remote: true }))]
    .filter(({ b, remote }) => b.toLowerCase().includes(search) || (remote && remoteLabel.includes(search)));

  const list = (chosen: string, disabled: boolean, onPick: (branch: string) => void) => (
    <View>
      {rows.length ? rows.map(({ b, remote }) => {
        const held = info.elsewhere?.[b];
        const hint = b === current ? m.native_folder_git_current() : remote ? remoteLabel : held ? m.native_git_elsewhere_tag({ pasta: basename(held.path) }) : '';
        const on = b === chosen;
        return (
          <Pressable key={`${remote}:${b}`} onPress={() => onPick(b)} disabled={disabled} accessibilityRole="button" accessibilityLabel={b}
            accessibilityHint={hint || undefined} accessibilityState={{ selected: on, disabled }} style={[styles.row, on && styles.on, disabled && styles.dim]}>
            <Text style={styles.rowName} numberOfLines={1} ellipsizeMode="middle">{b}</Text>
            {hint ? <Text style={styles.rowHint} numberOfLines={1}>{hint}</Text> : null}
            {on ? <Icon name="Check" size={16} color={theme.tokens.accent.base} /> : null}
          </Pressable>
        );
      }) : <Text style={styles.help}>{m.native_git_no_branch()}</Text>}
    </View>
  );

  const searchField = (
    <View style={styles.search}>
      <Icon name="Search" size={15} color={theme.tokens.text.muted} />
      <TextInput accessibilityLabel={m.native_git_branch_search()} placeholder={m.native_git_branch_search()} value={query} onChangeText={setQuery}
        autoCapitalize="none" autoCorrect={false} style={styles.searchInput} placeholderTextColor={theme.tokens.text.muted} />
    </View>
  );

  const button = (label: string, onPress: () => void, kind: 'primary' | 'danger' | 'plain' = 'plain', disabled = false) => (
    <Pressable onPress={onPress} disabled={disabled} accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ disabled, busy }}
      style={[styles.button, kind === 'primary' && styles.buttonPrimary, kind === 'danger' && styles.buttonDanger, disabled && styles.dim]}>
      {busy && kind !== 'plain' ? <ActivityIndicator size="small" color={theme.tokens.accent.base} /> : null}
      <Text style={[styles.buttonText, kind === 'primary' && styles.primaryText, kind === 'danger' && styles.dangerText]}>{label}</Text>
    </Pressable>
  );

  return (
    <ScrollView contentContainerStyle={styles.box} keyboardShouldPersistTaps="handled">
      <Pressable onPress={onBack} accessibilityRole="button" accessibilityLabel={m.comum_voltar()} style={styles.back}>
        <Icon name="ChevronLeft" size={16} color={theme.tokens.text.muted} />
        <Text style={styles.title} numberOfLines={1}>{m.native_folder_git_branch_title()}</Text>
      </Pressable>
      <TextTabs<Tab> label={m.native_folder_git_branch_title()} value={tab} onChange={(next) => { setTab(next); setQuery(''); clear(); }}
        options={[{ v: 'switch', label: m.native_folder_git_switch_tab() }, { v: 'create', label: m.native_folder_git_create_tab() }]} />

      {dirty ? <Text style={styles.warn}>{m.native_folder_git_dirty({ n: git.dirty ?? 0 })}</Text> : null}
      {failure ? <Text selectable style={styles.error} accessibilityRole="alert">{failure}</Text> : null}

      {pending ? (
        <View style={styles.notice} accessibilityRole="alert">
          <Text style={styles.noticeText}>{m.native_folder_git_sessions({ sessoes: sessions.join(', '), branch: pending.branch })}</Text>
          <View style={styles.line}>
            {button(m.native_folder_git_switch_anyway(), () => { void run(pending, true); }, 'danger', busy)}
            {button(m.comum_cancelar(), () => setPending(null))}
          </View>
        </View>
      ) : null}

      {elsewhere ? (
        <View style={styles.notice} accessibilityRole="alert">
          <Text style={styles.noticeText}>{m.native_git_elsewhere_body({ branch: elsewhere.branch })}</Text>
          <Text selectable style={styles.path}>{elsewhere.path}</Text>
          <Text style={styles.help}>{elsewhere.main ? m.native_git_elsewhere_main() : m.native_git_elsewhere_close_help({ branch: elsewhere.branch })}</Text>
          {button(m.native_git_elsewhere_use(), () => onOpenFolder(elsewhere.path), 'plain', busy)}
          {!elsewhere.main ? button(m.native_git_elsewhere_close(), () => { void closeElsewhere(elsewhere); }, 'primary', busy || dirty) : null}
          {button(m.comum_cancelar(), () => setElsewhere(null))}
        </View>
      ) : null}

      {tab === 'switch' ? (
        <>
          {searchField}
          {list(current, busy || dirty, pickBranch)}
        </>
      ) : (
        <>
          <TextInput accessibilityLabel={m.native_folder_git_name()} placeholder={m.native_folder_git_name()} value={name} onChangeText={setName}
            autoCapitalize="none" autoCorrect={false} editable={!busy} style={styles.input} placeholderTextColor={theme.tokens.text.muted} />
          <Pressable onPress={() => setCheckout((on) => !on)} disabled={busy} accessibilityRole="checkbox" accessibilityLabel={m.native_folder_git_checkout()}
            accessibilityState={{ checked: checkout, disabled: busy }} style={styles.check}>
            <Icon name={checkout ? 'SquareCheck' : 'Square'} size={18} color={checkout ? theme.tokens.accent.base : theme.tokens.text.muted} />
            <Text style={styles.rowName}>{m.native_folder_git_checkout()}</Text>
          </Pressable>
          {button(m.native_folder_git_create(), () => request({ kind: 'create', branch: typed }), 'primary', busy || !typed || (checkout && dirty))}
          <Text style={styles.label}>{m.native_folder_git_base()}</Text>
          {searchField}
          {list(base || current, busy, (b) => setBase(b === current ? '' : b))}
        </>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  box: { gap: theme.base.space[2] },
  back: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[2] },
  title: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  search: {
    minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.md, borderCurve: 'continuous', backgroundColor: superficie(theme),
  },
  searchInput: { flex: 1, minWidth: 0, minHeight: 44, fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  input: {
    minHeight: 44, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous', fontSize: theme.base.text.sm,
    color: theme.tokens.text.primary, backgroundColor: superficie(theme), borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  row: {
    minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
  },
  on: { backgroundColor: theme.tokens.accent.dim },
  dim: { opacity: 0.5 },
  rowName: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  rowHint: { flexShrink: 1, maxWidth: '45%', fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  check: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[1] },
  line: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  button: {
    flexGrow: 1, minHeight: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: theme.base.space[1.5],
    paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous', borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  buttonPrimary: { backgroundColor: theme.tokens.accent.dim, borderColor: theme.tokens.accent.base },
  buttonDanger: { borderColor: theme.tokens.status.warning },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  primaryText: { color: theme.tokens.accent.base },
  dangerText: { color: theme.tokens.status.warning },
  notice: {
    gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.status.warning,
  },
  noticeText: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  path: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  label: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary, paddingHorizontal: theme.base.space[1] },
  help: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary, paddingHorizontal: theme.base.space[1] },
  warn: { fontSize: theme.base.text.xs, color: theme.tokens.status.warning, paddingHorizontal: theme.base.space[1] },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error, paddingHorizontal: theme.base.space[1] },
}));
