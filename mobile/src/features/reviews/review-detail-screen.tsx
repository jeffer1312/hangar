import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Alert, FlatList, Linking, Platform, Text, View } from 'react-native';
import { Stack, useIsFocused, useLocalSearchParams, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import * as Clipboard from 'expo-clipboard';
import { canMergeReview, getReviewForServer, intlLocale, reviewActionForServer, reviewActivityText, reviewIdentity, type ReviewAction, type ReviewActivity,
  type ReviewDetail, type ReviewFile, type ReviewThread } from '@hangar/core';
import { Screen } from '../../ui/Screen';
import { HeaderBack, useGlassHeader } from '../../ui/glassHeader';
import { Chip } from '../../ui/Chip';
import { toast } from '../../ui/Toast';
import { useServers } from '../../stores/servers';
import { ReviewButton, ReviewNotice, ReviewSection, ReviewStatus, reviewNumber, styles } from './review-ui';
import { ReviewFileView, ReviewReplySheet, ReviewThreadView } from './review-thread-view';
import { useReviewResource } from './use-review-resource';
import { AssistantBubble } from '../../chat/AssistantBubble';
import { Sheet } from '../../ui/Sheet';
import { prepareReviewDraft, reviewThreadPrompt } from './review-draft';
import * as m from '../../paraglide/messages';

type Tab = 'threads' | 'files' | 'activity';
type Row = { kind: 'thread'; value: ReviewThread } | { kind: 'file'; value: ReviewFile } | { kind: 'activity'; value: ReviewActivity; index: number };
const approvals: Record<string, () => string> = { approved: m.native_review_approval_approved, changes: m.native_review_approval_changes,
  required: m.native_review_approval_required };
const approval = (state: string) => approvals[state]?.() ?? m.native_review_approval_none();
const blockers: Record<string, () => string> = {
  not_open: m.native_review_block_not_open, draft: m.native_review_block_draft, conflict: m.native_review_block_conflict,
  pipeline_failed: m.native_review_block_pipeline_failed, pipeline_running: m.native_review_block_pipeline_running,
  changes_requested: m.native_review_block_changes_requested, approval_required: m.native_review_block_approval_required,
  threads_open: m.native_review_block_threads_open, behind: m.native_review_block_behind, checking: m.native_review_block_checking,
  not_allowed: m.native_review_block_not_allowed, blocked: m.native_review_block_blocked,
};

export function ReviewDetailScreen() {
  const params = useLocalSearchParams<{ server: string; host: string; repo: string; number: string }>();
  const router = useRouter();
  const focused = useIsFocused();
  const header = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const server = useServers((state) => state.servers.find((item) => item.id === params.server));
  const target = useMemo(() => ({ host: params.host || '', repo: params.repo || '', number: Number(params.number) }), [params.host, params.repo, params.number]);
  const allowed = !!server && !server.invite && !server.removed && !server.disabled && Number.isSafeInteger(target.number) && target.number > 0;
  const key = JSON.stringify([server?.id, server?.baseUrl, server?.token, server?.invite, server?.removed, server?.disabled, reviewIdentity(target)]);
  const resource = useReviewResource<ReviewDetail>(key, (signal) => getReviewForServer(server!, target, signal), focused && allowed);
  const [tab, setTab] = useState<Tab>('threads');
  const [reply, setReply] = useState<ReviewThread | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [jobs, setJobs] = useState(false);
  const [showResolved, setShowResolved] = useState(false);
  const [sendChoice, setSendChoice] = useState<string | null>(null);
  const context = useRef({ key, focused, alive: true, writing: false });
  if (context.current.key !== key || context.current.focused !== focused) context.current = { key, focused, alive: true, writing: false };
  const owner = context.current;
  useEffect(() => {
    owner.alive = true;
    setReply(null); setSendChoice(null); setBusy(false); setError(''); setTab('threads'); setShowResolved(false); setJobs(false);
    return () => { owner.alive = false; };
  }, [owner]);
  const detail = resource.data;
  const act = useCallback(async (action: ReviewAction, options: { thread?: string; body?: string } = {}): Promise<boolean> => {
    if (!allowed || !server || !focused || context.current !== owner || !owner.alive || owner.writing || !detail) return false;
    if (action === 'merge' && !canMergeReview(detail)) return false;
    owner.writing = true; setBusy(true); setError('');
    try {
      await reviewActionForServer(server, target, action, options, () => context.current === owner && owner.alive && owner.focused);
      if (context.current !== owner || !owner.alive) return false;
      await resource.refresh();
      if (context.current !== owner || !owner.alive) return false;
      toast.ok(m.mobile_review_action_done());
      return true;
    } catch (cause) {
      if (context.current === owner && owner.alive) {
        const status = cause && typeof cause === 'object' && 'status' in cause ? Number(cause.status) : 0;
        const message = status >= 400 && status < 500 && cause instanceof Error ? cause.message : m.mobile_review_action_uncertain();
        setError(message);
        if (action !== 'reply') toast.erro(message);
      }
      return false;
    } finally {
      owner.writing = false;
      if (context.current === owner && owner.alive) setBusy(false);
    }
  }, [allowed, server, focused, owner, detail, target, resource.refresh]);
  const copy = useCallback((thread: ReviewThread) => {
    if (!detail) return;
    void Clipboard.setStringAsync(reviewThreadPrompt(detail, [thread])).then(() => toast.ok(m.mobile_review_copied()), () => toast.erro(m.erro_desconhecido()));
  }, [detail]);
  const putInSession = async (name: string, text: string) => {
    if (!server || !allowed || context.current !== owner || !owner.alive || owner.writing) return;
    owner.writing = true; setBusy(true); setSendChoice(null);
    try {
      await prepareReviewDraft(server, name, text, () => context.current === owner && owner.alive && owner.focused);
      if (context.current !== owner || !owner.alive) return;
      toast.ok(m.native_review_sent({ nome: name }));
      router.push({ pathname: '/s/[server]/[name]', params: { server: server.id, name } } as never);
    } catch (cause) {
      if (context.current === owner && owner.alive) toast.erro(cause instanceof Error ? cause.message : m.erro_desconhecido());
    } finally {
      owner.writing = false;
      if (context.current === owner && owner.alive) setBusy(false);
    }
  };
  const sendAll = () => {
    if (!detail) return;
    const text = reviewThreadPrompt(detail, detail.threads.filter((thread) => !thread.resolved));
    if (detail.sessions.length === 1) void putInSession(detail.sessions[0], text);
    else if (detail.sessions.length > 1) setSendChoice(text);
    else void Clipboard.setStringAsync(text).then(() => toast.ok(m.native_review_copied()));
  };
  const resolve = useCallback((thread: ReviewThread) => { void act('resolve', { thread: thread.id }); }, [act]);
  const openReply = useCallback((thread: ReviewThread) => setReply(thread), []);
  const confirm = (action: 'approve' | 'merge' | 'auto_merge', label: string) => Alert.alert(label,
    `${detail?.repo} ${detail ? reviewNumber(detail) : ''}`, [
      { text: m.native_cancel(), style: 'cancel' }, { text: label, onPress: () => { void act(action); } },
    ]);
  const rows = useMemo((): Row[] => !detail ? [] : tab === 'threads' ? detail.threads.filter((value) => showResolved || !value.resolved).map((value) => ({ kind: 'thread', value }))
    : tab === 'files' ? detail.files.map((value) => ({ kind: 'file', value }))
      : detail.activity.map((value, index) => ({ kind: 'activity', value, index })), [detail, tab, showResolved]);
  const renderItem = useCallback(({ item }: { item: Row }) => item.kind === 'thread'
    ? <ReviewThreadView thread={item.value} serverId={params.server} busy={busy} onReply={openReply} onResolve={resolve} onCopy={copy} />
    : item.kind === 'file' ? <ReviewFileView file={item.value} />
      : <View style={styles.section}><Text style={styles.heading}>{item.value.who}</Text><AssistantBubble text={reviewActivityText(item.value)} serverId={params.server} />
        <Text style={styles.meta}>{Number.isFinite(Date.parse(item.value.at)) ? new Date(item.value.at).toLocaleString(intlLocale()) : ''}</Text></View>,
  [params.server, busy, openReply, resolve, copy]);
  const ready = !!detail && canMergeReview(detail);
  const reasons = detail?.merge.blockers.map((code) => blockers[code]?.() ?? m.native_review_block_checking()) ?? [];
  if (detail && !ready && (detail.pipeline === 'running' || detail.pipeline === 'pending') && !reasons.includes(m.native_review_block_pipeline_running())) reasons.unshift(m.native_review_block_pipeline_running());

  return <Screen edges={['bottom', 'left', 'right']}>
    <Stack.Screen options={{ ...header, title: detail ? `${detail.provider === 'gitlab' ? m.native_review_kind_mr() : m.native_review_kind_pr()} ${reviewNumber(detail)}` : m.native_review_title(),
      headerLeft: router.canGoBack() ? undefined : () => <HeaderBack onPress={() => router.replace('/reviews' as never)} />,
      headerRight: () => <ReviewButton icon="RefreshCw" iconOnly label={m.native_review_refresh()} onPress={() => { void resource.refresh(); }} /> }} />
    <FlatList data={rows} renderItem={renderItem} keyExtractor={(item) => item.kind === 'thread' ? `thread:${item.value.id}` : item.kind === 'file' ? `file:${item.value.path}` : `activity:${item.index}`}
      style={styles.page} contentContainerStyle={[styles.content, Platform.OS !== 'ios' ? { paddingTop: headerHeight } : null]}
      contentInsetAdjustmentBehavior="automatic" refreshing={resource.loading && !!detail} onRefresh={() => { void resource.refresh(); }}
      ListHeaderComponent={detail ? <View style={styles.section}>
        <Text style={styles.meta}>{server?.label} · {detail.repo}</Text>
        <Text style={styles.title}>{detail.title}</Text>
        <Text style={styles.meta}>{detail.source} {m.native_review_into()} {detail.target} · {m.native_review_by({ autor: detail.author })}</Text>
        <View style={styles.wrap}><ReviewStatus pipeline={detail.pipeline} /><Chip tone={detail.approvals.state === 'approved' ? 'success' : 'neutral'}>{approval(detail.approvals.state)}</Chip>
          {detail.draft ? <Chip>{m.native_review_state_draft()}</Chip> : null}
          {detail.conflicts ? <Chip tone="error">{m.native_review_conflict_yes()}</Chip> : null}</View>
        <ReviewButton label={m.native_review_open_web({ site: detail.provider === 'gitlab' ? 'GitLab' : 'GitHub' })} icon="ExternalLink" onPress={() => { void Linking.openURL(detail.url); }} />
        {detail.jobs.length ? <ReviewButton label={`${m.native_review_card_pipeline()} (${detail.jobs.length})`} icon="List" onPress={() => setJobs((value) => !value)} /> : null}
        {jobs ? <ReviewSection title={m.native_review_card_pipeline()}>{detail.jobs.map((job, index) => <View key={index} style={styles.wrap}><Text style={styles.meta}>{job.name}</Text><ReviewStatus pipeline={job.status} /></View>)}</ReviewSection> : null}
        <View style={styles.wrap}>
          {detail.approvals.can_approve ? <ReviewButton label={m.native_review_approve()} icon="Check" disabled={busy} onPress={() => confirm('approve', m.native_review_approve())} /> : null}
          {detail.merge.can_auto && !detail.merge.auto_enabled ? <ReviewButton label={m.native_review_auto_merge()} disabled={busy} onPress={() => confirm('auto_merge', m.native_review_auto_merge())} /> : null}
          <ReviewButton label={m.native_review_merge()} primary icon="GitMerge" disabled={busy || !ready} onPress={() => confirm('merge', m.native_review_merge())} />
        </View>
        {tab === 'threads' && detail.threads.some((thread) => !thread.resolved) ? <ReviewButton
          label={detail.sessions.length ? m.native_review_send() : m.native_review_copy()} icon={detail.sessions.length ? 'MessageSquare' : 'Copy'}
          disabled={busy} onPress={sendAll} /> : null}
        <Text style={styles.muted}>{detail.merge.auto_enabled ? m.native_review_auto_on() : ready ? m.native_review_ready() : m.native_review_blocked({ motivos: reasons.join(', ') || m.native_review_block_checking() })}</Text>
        {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
        <View style={styles.wrap}>
          {(['threads', 'files', 'activity'] as const).map((value) => <ReviewButton key={value} primary={tab === value} selected={tab === value} onPress={() => setTab(value)}
            label={value === 'threads' ? `${m.native_review_tab_threads()} (${detail.open_threads ?? 0})` : value === 'files' ? `${m.native_review_tab_files()} (${detail.files.length})` : m.native_review_tab_activity()} />)}
        </View>
        {tab === 'threads' && detail.threads.some((thread) => thread.resolved) ? <ReviewButton
          label={showResolved ? m.native_review_hide_resolved() : m.native_review_show_resolved({ n: String(detail.threads.filter((thread) => thread.resolved).length) })}
          onPress={() => setShowResolved((value) => !value)} /> : null}
      </View> : null}
      ListEmptyComponent={!allowed ? <ReviewNotice text={m.native_review_list_unavailable()} /> : !detail ? <ReviewNotice text={resource.error || m.native_loading()} loading={resource.loading} retry={resource.error ? () => { void resource.refresh(); } : undefined} />
        : <ReviewNotice text={tab === 'threads' ? m.native_review_threads_empty() : tab === 'files' ? m.native_review_files_empty() : m.native_review_activity_empty()} />} />
    {reply && detail ? <ReviewReplySheet key={reply.id} thread={reply} error={error} onClose={() => setReply(null)} onSend={(body) => act('reply', { thread: reply.id, body })} /> : null}
    {sendChoice && detail ? <Sheet open onDismiss={() => setSendChoice(null)} sizes={['medium']}>
      <View style={styles.content}><Text style={styles.title}>{m.native_review_send()}</Text>
        {detail.sessions.map((name) => <ReviewButton key={name} label={name} icon="MessageSquare" onPress={() => { void putInSession(name, sendChoice); }} />)}
      </View>
    </Sheet> : null}
  </Screen>;
}
