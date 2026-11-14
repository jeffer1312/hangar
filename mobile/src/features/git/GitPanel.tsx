import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, FlatList, Platform, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { basename, checkoutBranch, commitFiles, discardFile, getBranches, getChangedFiles, getCommitDiff, getFileDiff, getGitLog, getSessions, gitAction, gitCreateBranch, gitPush, relativeTime, type BranchInfo, type ChangedFile, type GitCommit, type Server, type SessionInfo } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { MultilineInput } from '../../ui/MultilineInput';
import { haptic } from '../../ui/haptics';
import { PatchLine } from './patch-line';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

type LocalBranches = BranchInfo & { elsewhere?: Record<string, { path: string; main: boolean }> };
type Identity = { cwd: string; jsonl: string | null; provider: SessionInfo['provider']; headless: boolean };
type Target = { identity: Identity; branches: LocalBranches };
type Repo = Target & { files: ChangedFile[]; ahead: number | null; behind: number | null };
type Write = <T>(call: () => Promise<T>) => Promise<T>;
const elsewhereFor = (branches: LocalBranches, branch: string) => branches.elsewhere && Object.hasOwn(branches.elsewhere, branch) ? branches.elsewhere[branch] : undefined;
const sameTarget = (a: Target, b: Target) => a.identity.cwd === b.identity.cwd && a.identity.jsonl === b.identity.jsonl && a.identity.provider === b.identity.provider && a.identity.headless === b.identity.headless && a.branches.current === b.branches.current;
const reason = (error: unknown) => error instanceof Error ? error.message : m.erro_desconhecido();
class LocalGitError extends Error {}
const failure = (error: unknown) => error instanceof Error ? error : new LocalGitError(m.erro_desconhecido());
function GitFailure({ error }: { error: Error }) {
  const detail = error instanceof LocalGitError && error.cause ? reason(error.cause) : error.message;
  const summary = error instanceof LocalGitError ? error.message : /not a git repository/i.test(detail) ? m.native_git_not_repo() : m.mobile_git_operation_failed();
  return <View>
    <Text selectable accessibilityRole="alert" style={styles.error}>{summary}</Text>
    {detail !== summary ? <Text selectable style={styles.errorDetail}>{detail}</Text> : null}
  </View>;
}
function check(result: { ok: boolean; output?: string }) {
  if (result.ok !== true) throw new Error(result.output || m.mobile_git_operation_failed());
  return result;
}
function useDeadline() {
  const pending = useRef(new Set<() => void>());
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; for (const cancel of [...pending.current]) cancel(); };
  }, []);
  return useCallback(<T,>(call: () => Promise<T>, write = false): Promise<T> => new Promise<T>((resolve, reject) => {
    if (!mounted.current) { reject(new LocalGitError(m.mobile_git_context_changed())); return; }
    let finished = false;
    const finish = (settle: () => void) => {
      if (finished) return;
      finished = true; clearTimeout(timer); pending.current.delete(cancel); settle();
    };
    const cancel = () => finish(() => reject(new LocalGitError(m.mobile_git_context_changed())));
    // O prazo solta a interface; não cancela a operação HTTP que já foi enviada.
    const timer = setTimeout(() => finish(() => reject(new LocalGitError(write ? m.mobile_git_write_timeout() : m.mobile_git_read_timeout()))), write ? 60000 : 20000);
    pending.current.add(cancel);
    try { void call().then((value) => finish(() => resolve(value)), (error) => finish(() => reject(error))); }
    catch (error) { finish(() => reject(error)); }
  }), []);
}
function GitButton({ label, icon, onPress, disabled = false, flexible = false }: { label: string; icon?: IconName; onPress: () => void; disabled?: boolean; flexible?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ disabled }} disabled={disabled} onPress={onPress} style={[styles.button, flexible && styles.grow, disabled && styles.disabled]}>
    {icon ? <Icon name={icon} /> : null}<Text numberOfLines={2} ellipsizeMode="tail" style={[styles.text, styles.shrink]}>{label}</Text>
  </Pressable>;
}

function fileStatus(code: string) {
  const status = code.trim();
  if (status === '??' || status.startsWith('A')) return m.git_tag_novo();
  if (status.includes('D')) return m.git_tag_del();
  if (status.startsWith('R')) return m.git_tag_ren();
  if (status.includes('U')) return m.git_tag_conflito();
  return m.git_tag_mod();
}

function GitDiff({ server, name, target, kind, revision }: { server: Server; name: string; target: string; kind: 'file' | 'commit'; revision: number }) {
  const deadline = useDeadline();
  const [content, setContent] = useState<{ diff: string; truncated: boolean } | null>(null);
  const [error, setError] = useState<Error | null>(null);
  useEffect(() => {
    let active = true;
    setContent(null); setError(null);
    const read = deadline<{ diff: string; truncated: boolean }>(() => kind === 'file' ? getFileDiff(name, target, server) : getCommitDiff(name, target, server));
    void read.then((value) => { if (active) setContent(value); }, (error) => { if (active) setError(failure(error)); });
    return () => { active = false; };
  }, [server, name, target, kind, revision, deadline]);
  return <View style={styles.diff}>
    <Text selectable numberOfLines={2} ellipsizeMode="middle" style={[styles.meta, styles.shrink]}>{target}</Text>
    {error ? <ScrollView style={styles.output} nestedScrollEnabled><GitFailure error={error} /></ScrollView>
      : !content ? <Text style={styles.meta}>{m.native_git_diff_loading()}</Text>
      : <>
        {content.truncated ? <Text style={styles.meta}>{m.git_diff_cortado()}</Text> : null}
        <FlatList data={content.diff ? content.diff.split('\n') : []} keyExtractor={(_, index) => String(index)}
          ListEmptyComponent={<Text style={styles.meta}>{m.native_git_diff_empty()}</Text>}
          renderItem={({ item }) => <PatchLine line={item} />} />
      </>}
  </View>;
}

function GitHistory({ server, name, revision }: { server: Server; name: string; revision: number }) {
  const deadline = useDeadline();
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [limit, setLimit] = useState(30);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);
  const [selected, setSelected] = useState<GitCommit | null>(null);
  const pending = useRef(false);
  useEffect(() => {
    let active = true;
    pending.current = true; setLoading(true); setError(null);
    void deadline(() => getGitLog(name, undefined, limit, server)).then((value) => {
      if (active) setCommits(value.commits);
    }, (error) => { if (active) setError(failure(error)); }).finally(() => {
      if (active) { pending.current = false; setLoading(false); }
    });
    return () => { active = false; };
  }, [server, name, limit, revision, deadline]);
  return <View style={styles.root}>
    {loading ? <Text style={styles.meta}>{m.native_git_loading()}</Text> : null}
    {error ? <ScrollView style={styles.output} nestedScrollEnabled><GitFailure error={error} /></ScrollView> : null}
    {selected ? <>
      <GitButton label={m.native_git_commit_close()} icon="X" onPress={() => setSelected(null)} />
      <ScrollView style={styles.output} contentContainerStyle={styles.commitContent} nestedScrollEnabled>
        <Text selectable style={styles.text}>{selected.subject || m.native_git_no_subject()}</Text>
        {selected.body ? <Text selectable style={styles.meta}>{selected.body}</Text> : null}
      </ScrollView>
      <GitDiff key={selected.hash} server={server} name={name} target={selected.hash} kind="commit" revision={revision} />
    </> : <>
      <FlatList data={commits} keyExtractor={(item) => item.hash}
        ListEmptyComponent={!loading && !error ? <Text style={styles.meta}>{m.native_git_no_commits()}</Text> : null}
        renderItem={({ item }) => <Pressable accessibilityRole="button" accessibilityLabel={m.git_titulo_commit({ short: item.short, subject: item.subject || m.native_git_no_subject() })} onPress={() => { haptic.selection(); setSelected(item); }} style={styles.commitRow}>
          <Text style={[styles.text, styles.commitSubject]}>{item.subject || m.native_git_no_subject()}</Text>
          <Text style={styles.meta}>{item.short} · {item.author} · {relativeTime(item.ts)}</Text>
          {item.refs ? <Text style={styles.meta}>{item.refs}</Text> : null}
          {item.local ? <Text style={styles.meta}>{m.native_git_commit_local()}</Text> : null}
        </Pressable>} />
      {commits.length >= limit ? <GitButton label={m.native_git_more()} disabled={loading || !!error} onPress={() => {
        if (pending.current) return;
        pending.current = true; setLimit((value) => value * 2);
      }} /> : null}
    </>}
  </View>;
}

export function GitPanel({ server, name }: { server: Server; name: string }) {
  const deadline = useDeadline();
  const { theme } = useUnistyles();
  const alive = useRef(true);
  const generation = useRef(0);
  const [repo, setRepo] = useState<Repo | null>(null);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState<Error | null>(null);
  const [selection, setSelection] = useState<Record<string, boolean>>({});
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  const [operationError, setOperationError] = useState<Error | null>(null);
  const [result, setResult] = useState('');
  const [tab, setTab] = useState<'changes' | 'history' | 'branches'>('changes');
  const [branchFilter, setBranchFilter] = useState('');
  const [branchName, setBranchName] = useState('');
  const [filePath, setFilePath] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const operation = useRef<{ started: boolean } | null>(null);
  const current = () => {
    const state = useServers.getState();
    const saved = state.servers.find((s) => s.id === server.id);
    return alive.current && state.ready && !!saved && !saved.removed && !saved.invite && !saved.inviteEnded && saved.baseUrl === server.baseUrl && saved.token === server.token;
  };
  const readTarget = useCallback(async (): Promise<Target> => {
    const [sessions, branches] = await Promise.all([deadline(() => getSessions(server)), deadline(() => getBranches(name, server))]);
    const session = sessions.find((item) => item.name === name);
    const cwd = session?.git_cwd ?? session?.cwd;
    if (!session || !cwd || session.worktree_gone) throw new LocalGitError(m.mobile_git_context_changed());
    return { identity: { cwd, jsonl: session.jsonl ?? null, provider: session.provider, headless: session.headless ?? false }, branches };
  }, [name, server, deadline]);
  const reload = useCallback(async () => {
    const turn = ++generation.current;
    setLoading(true); setReadError(null); setRevision((value) => value + 1);
    try {
      const target = await readTarget();
      const [files, log] = await Promise.all([deadline(() => getChangedFiles(name, server)), deadline(() => getGitLog(name, undefined, 1, server))]);
      const confirmed = await readTarget();
      if (!alive.current || turn !== generation.current) return;
      if (!sameTarget(target, confirmed)) throw new LocalGitError(m.mobile_git_context_changed());
      setRepo({ ...target, files: files.files, ahead: log.ahead, behind: log.behind });
      setFilePath((path) => files.files.some((file) => file.path === path) ? path : null);
      setSelection((previous) => Object.fromEntries(files.files.map((file) => [file.path, Object.hasOwn(previous, file.path) ? previous[file.path] : true])));
    } catch (error) {
      if (alive.current && turn === generation.current) setReadError(failure(error));
    } finally {
      if (alive.current && turn === generation.current) setLoading(false);
    }
  }, [name, server, readTarget, deadline]);
  useEffect(() => {
    alive.current = true;
    void reload();
    return () => { alive.current = false; generation.current++; operation.current = null; };
  }, [reload]);
  const request = (work: (write: Write) => Promise<string>, confirmation?: { title: string; body: string; destructive?: boolean }, notify = true) => {
    if (!current() || operation.current || loading || readError || !repo) return;
    const ticket = { started: false };
    operation.current = ticket; setBusy(true);
    const cancel = () => {
      if (!alive.current || operation.current !== ticket || ticket.started) return;
      operation.current = null; setBusy(false);
    };
    const run = async () => {
      if (!current() || operation.current !== ticket || ticket.started) return;
      ticket.started = true; generation.current++;
      setOperationError(null); setResult('');
      try {
        const write: Write = async (call) => {
          const target = await readTarget();
          if (!current() || operation.current !== ticket || !sameTarget(repo, target)) throw new LocalGitError(m.mobile_git_context_changed());
          return deadline(call, true);
        };
        const output = await work(write);
        if (current()) { setResult(output); if (notify) haptic.success(); }
      } catch (error) {
        if (current()) { setOperationError(failure(error)); if (notify) haptic.error(); }
      } finally {
        // Uma escrita recusada ainda pode ter alterado o stage ou criado uma branch.
        if (current()) await reload();
        if (alive.current && operation.current === ticket) { operation.current = null; setBusy(false); }
      }
    };
    if (confirmation) Alert.alert(confirmation.title, confirmation.body, [
      { text: m.comum_cancelar(), style: 'cancel', onPress: cancel },
      { text: m.comum_confirmar(), style: confirmation.destructive ? 'destructive' : 'default', onPress: () => { void run(); } },
    ], { cancelable: true, onDismiss: cancel });
    else void run();
  };
  const isSelected = (path: string) => Object.hasOwn(selection, path) && selection[path] === true;
  const selectedPaths = repo?.files.filter((file) => isSelected(file.path)).map((file) => file.path) ?? [];
  const publishConfirmation = (action: string) => ({ title: m.mobile_git_publish_title(), body: m.mobile_git_publish_body({ action }) });
  const commit = (publish: boolean) => {
    const text = message.trim();
    if (!text || !selectedPaths.length) return;
    request(async (write) => {
      check(await write(() => commitFiles(name, text, selectedPaths, undefined, server)));
      if (current()) setMessage('');
      if (publish && current()) {
        try { check(await write(() => gitPush(name, server))); }
        catch (error) { throw new LocalGitError(`${m.native_git_committed()} ${m.native_git_sync_failed({ op: m.native_git_push() })}`, { cause: error }); }
        return `${m.native_git_committed()} ${m.native_git_pushed()}`;
      }
      return m.native_git_committed();
    }, publish ? publishConfirmation(m.native_git_commit_push()) : undefined);
  };
  const branchProblem = (branches: LocalBranches, files: ChangedFile[], branch?: string) => {
    if (branches.dirty || files.length) return m.git_working_tree_suja();
    const elsewhere = branch ? elsewhereFor(branches, branch) : undefined;
    return elsewhere ? `${m.native_git_elsewhere_title({ branch: branch! })}\n${m.native_git_elsewhere_body({ branch: branch! })}\n${elsewhere.path}` : '';
  };
  const changeBranch = (branch: string, create: boolean) => {
    if (!repo || !branch || operation.current) return;
    const problem = branchProblem(repo.branches, repo.files, create ? undefined : branch);
    if (problem) { setOperationError(new LocalGitError(problem)); haptic.error(); return; }
    request(async (write) => {
      const [freshBranches, freshFiles] = await Promise.all([deadline(() => getBranches(name, server)), deadline(() => getChangedFiles(name, server))]);
      if (!current()) return '';
      const problem = branchProblem(freshBranches, freshFiles.files, create ? undefined : branch);
      if (problem) throw new LocalGitError(problem);
      if (create) {
        check(await write(() => gitCreateBranch(name, { name: branch, switch_after: true }, server)));
        if (current()) setBranchName('');
        return m.native_git_branch_created({ n: branch });
      }
      const response = await write(() => checkoutBranch(name, branch, server));
      if ((response as { ok?: boolean }).ok === false) throw new Error(response.output || m.mobile_git_operation_failed());
      return response.output || branch;
    }, { title: m.mobile_git_checkout_title({ branch }), body: m.mobile_git_branch_body() });
  };
  const unavailable = busy || loading || !!readError || !repo;
  const commitDisabled = unavailable || !message.trim() || !selectedPaths.length;
  return <View style={styles.root}>
    <View style={styles.bar}>
      <Text numberOfLines={2} ellipsizeMode="tail" style={[styles.text, styles.grow]}>{repo?.branches.current ?? m.ctx_sem_branch()}</Text>
      <GitButton label={m.native_git_reload()} icon="RefreshCw" disabled={busy} onPress={() => void reload()} />
    </View>
    {repo ? <Text style={styles.meta}>{repo.ahead === null ? '' : m.native_git_ahead({ n: repo.ahead })} {repo.behind === null ? '' : m.native_git_behind({ n: repo.behind })}</Text> : null}
    <View style={styles.bar}>
      {([
        ['changes', m.native_git_changes()],
        ['history', m.native_git_history()],
        ['branches', m.git_aba_branches()],
      ] as const).map(([value, label]) => <Pressable key={value} accessibilityRole="tab" accessibilityState={{ selected: tab === value }} accessibilityLabel={label} onPress={() => { if (tab !== value) { haptic.selection(); setTab(value); } }} style={styles.button}>
        <Text style={[styles.text, tab === value && styles.selected]}>{label}</Text>
      </Pressable>)}
    </View>
    <View style={styles.bar}>
      {(['fetch', 'pull', 'status'] as const).map((action) => <GitButton key={action} label={action === 'fetch' ? m.native_git_fetch() : action === 'pull' ? m.native_git_pull() : m.mobile_git_status()} disabled={unavailable} onPress={() => request(async (write) => {
        const response = check(await write(() => gitAction(name, action, server)));
        return response.output || (action === 'fetch' ? m.native_git_fetched() : action === 'pull' ? m.native_git_pulled() : m.mobile_git_status());
      }, undefined, action !== 'status')} />)}
      <GitButton label={m.native_git_push()} icon="UploadIcon" disabled={unavailable} onPress={() => request(async (write) => { check(await write(() => gitPush(name, server))); return m.native_git_pushed(); }, publishConfirmation(m.native_git_push()))} />
    </View>
    {loading ? <View style={styles.bar}><ActivityIndicator color={theme.tokens.text.muted} /><Text style={styles.text}>{m.native_git_loading()}</Text></View> : null}
    {readError || operationError || result ? <ScrollView style={styles.output} nestedScrollEnabled>
      {readError ? <GitFailure error={readError} /> : null}
      {operationError ? <GitFailure error={operationError} /> : null}
      {result ? <Text selectable style={styles.meta}>{result}</Text> : null}
    </ScrollView> : null}
    {repo && !readError && tab === 'history' ? <GitHistory server={server} name={name} revision={revision} /> : null}
    {repo && !readError && tab === 'branches' ? <>
      <TextInput accessibilityLabel={m.native_git_branch_search()} placeholder={m.native_git_branch_search()} value={branchFilter} onChangeText={setBranchFilter} style={[styles.input, styles.text]} />
      <FlatList data={[...repo.branches.branches.map((branch) => ({ branch, remote: false })), ...(repo.branches.remotes ?? []).map((branch) => ({ branch, remote: true }))].filter(({ branch }) => branch.toLowerCase().includes(branchFilter.toLowerCase()))}
        keyExtractor={(item) => `${item.remote}:${item.branch}`} keyboardShouldPersistTaps="handled"
        ListEmptyComponent={<Text style={styles.meta}>{m.native_git_no_branch()}</Text>}
        renderItem={({ item }) => <View style={styles.row}>
          <GitButton label={item.branch} icon="GitBranch" flexible disabled={unavailable || item.branch === repo.branches.current} onPress={() => changeBranch(item.branch, false)} />
          {item.branch === repo.branches.current ? <Text style={styles.meta}>{m.native_git_current()}</Text> : null}
          {item.remote ? <Text style={styles.meta}>{m.native_git_remote_badge()}</Text> : null}
          {elsewhereFor(repo.branches, item.branch) ? <Text selectable numberOfLines={3} ellipsizeMode="middle" style={[styles.meta, styles.grow]}>{m.native_git_elsewhere_tag({ pasta: elsewhereFor(repo.branches, item.branch)!.path })}</Text> : null}
        </View>} />
      <TextInput accessibilityLabel={m.git_branch_nome_placeholder()} placeholder={m.git_branch_nome_placeholder()} value={branchName} onChangeText={setBranchName} editable={!busy} autoCapitalize="none" style={[styles.input, styles.text]} />
      <GitButton label={m.native_git_branch_create({ n: branchName.trim() })} icon="GitBranch" disabled={unavailable || !branchName.trim()} onPress={() => changeBranch(branchName.trim(), true)} />
    </> : null}
    {repo && !readError && tab === 'changes' ? filePath ? <>
      <View style={styles.bar}>
        <GitButton label={m.arq_aba()} icon="ChevronLeft" onPress={() => setFilePath(null)} />
        <GitButton label={m.native_git_discard_hint()} icon="Trash2" disabled={unavailable} onPress={() => request(async (write) => {
          check(await write(() => discardFile(name, filePath, server)));
          return m.native_git_discarded({ path: filePath });
        }, { title: m.native_git_discard_title({ path: filePath }), body: m.native_git_discard_body({ path: filePath }), destructive: true })} />
      </View>
      <GitDiff server={server} name={name} target={filePath} kind="file" revision={revision} />
    </> : <>
      <Text style={styles.meta}>{m.native_git_picked({ n: selectedPaths.length, total: repo.files.length })}</Text>
      <FlatList data={repo.files} keyExtractor={(file) => file.path} extraData={selection}
        ListEmptyComponent={!loading && !readError ? <Text style={styles.meta}>{m.native_git_clean()}</Text> : null}
        renderItem={({ item }) => {
          const path = item.path.replace(/\/+$/, '');
          const directory = path.slice(0, path.lastIndexOf('/') + 1);
          return <View style={styles.row}>
          <Pressable disabled={busy} accessibilityRole="checkbox" accessibilityLabel={m.native_git_include({ path: item.path })} accessibilityState={{ checked: isSelected(item.path) }}
            onPress={() => { haptic.selection(); setSelection((previous) => ({ ...previous, [item.path]: !(Object.hasOwn(previous, item.path) && previous[item.path]) })); }} style={[styles.button, styles.checkbox]}>
            <Icon name={isSelected(item.path) ? 'SquareCheck' : 'Square'} />
          </Pressable>
          <Pressable accessibilityRole="button" accessibilityLabel={item.path} accessibilityHint={m.git_ver_diff()} onPress={() => { if (filePath !== item.path) { haptic.selection(); setFilePath(item.path); } }} style={[styles.button, styles.grow]}>
            <View style={styles.grow}>
              <Text numberOfLines={1} ellipsizeMode="tail" style={[styles.text, styles.shrink, styles.filename]}>{basename(item.path)}</Text>
              {directory ? <Text numberOfLines={1} ellipsizeMode="middle" style={[styles.meta, styles.shrink]}>{directory}</Text> : null}
            </View>
          </Pressable>
          <View style={styles.fileStats}>
            <Text numberOfLines={1} ellipsizeMode="tail" style={styles.meta}>{fileStatus(item.code)}</Text>
            <Text><Text style={styles.add}>{item.added === null ? '' : `+${item.added}`}</Text> <Text style={styles.remove}>{item.removed === null ? '' : `−${item.removed}`}</Text></Text>
          </View>
        </View>; }} />
      <View style={styles.composer}>
        <MultilineInput accessibilityLabel={m.native_git_message()} placeholder={m.native_git_message()} value={message} onChangeText={setMessage} editable={!busy} maxHeight={100} style={styles.input} />
        <View style={styles.bar}>
          <GitButton label={m.native_git_commit()} icon="GitCommitHorizontal" disabled={commitDisabled} onPress={() => commit(false)} />
          <GitButton label={m.native_git_commit_push()} icon="UploadIcon" disabled={commitDisabled} onPress={() => commit(true)} />
        </View>
      </View>
    </> : null}
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1, minHeight: 0, minWidth: 0 },
  output: { maxHeight: 144, flexGrow: 0, flexShrink: 1 },
  diff: { flex: 2, minHeight: 100 },
  disabled: { opacity: 0.5 },
  selected: { color: theme.tokens.accent.base, fontWeight: theme.base.weight.semibold },
  composer: { padding: theme.base.space[2], borderTopWidth: 1, borderColor: theme.tokens.border.subtle },
  input: { minHeight: 44, padding: theme.base.space[2], borderWidth: 1, borderColor: theme.tokens.border.subtle, borderRadius: theme.base.radius.md },
  bar: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3] },
  commitContent: { paddingHorizontal: theme.base.space[3] },
  commitSubject: { paddingHorizontal: theme.base.space[1] },
  commitRow: { minHeight: 44, padding: theme.base.space[2], borderBottomWidth: 1, borderColor: theme.tokens.border.subtle },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], paddingHorizontal: theme.base.space[2], borderBottomWidth: 1, borderColor: theme.tokens.border.subtle },
  button: { minHeight: 44, padding: theme.base.space[2], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  grow: { flex: 1, minWidth: 0, flexShrink: 1 },
  shrink: { minWidth: 0, flexShrink: 1 },
  checkbox: { minWidth: 44 },
  filename: { fontWeight: theme.base.weight.semibold },
  fileStats: { maxWidth: '40%', flexShrink: 1 },
  text: { color: theme.tokens.text.primary, fontSize: theme.base.text.sm },
  meta: { color: theme.tokens.text.secondary, fontSize: theme.base.text.xs, padding: theme.base.space[1] },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm, padding: theme.base.space[3] },
  errorDetail: { color: theme.tokens.text.secondary, fontSize: theme.base.text.xs, paddingHorizontal: theme.base.space[3], paddingBottom: theme.base.space[3] },
  add: { color: theme.tokens.status.success, fontSize: theme.base.text.xs },
  remove: { color: theme.tokens.status.error, fontSize: theme.base.text.xs },
}));
