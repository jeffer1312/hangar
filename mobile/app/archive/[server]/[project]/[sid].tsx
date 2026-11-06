import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import type { Edge } from 'react-native-safe-area-context';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import {
  basename,
  cotaDaConta,
  getArchiveFolder,
  getArchiveHistory,
  getCodexAccountsForServer,
  getEnginesForServer,
  janelaEsgotada,
  listClaudeConfigsForServer,
  probeServerResponse,
  resumeArchivedConversation,
  sendInputForServer,
  takeEntry,
  type ArchiveEntry,
  type ChatEvent,
  type CodexAccount,
  type ConfigDirInfo,
  type CotaContaResumo,
  type Motor,
} from '@hangar/core';
import { useServers } from '../../../../src/stores/servers';
import { readDraft, writeDraft, type ConversationDraft } from '../../../../src/stores/drafts';
import { Screen } from '../../../../src/ui/Screen';
import { Glass } from '../../../../src/ui/Glass';
import { Icon } from '../../../../src/ui/Icon';
import { HeaderBack, useGlassHeader } from '../../../../src/ui/glassHeader';
import { MultilineInput } from '../../../../src/ui/MultilineInput';
import { ProviderGlyph } from '../../../../src/ui/ProviderGlyph';
import { toast } from '../../../../src/ui/Toast';
import { Sheet } from '../../../../src/ui/Sheet';
import { MessageList } from '../../../../src/chat/MessageList';
import { CwdPicker } from '../../../../src/features/create/CwdPicker';
import { QuietPill } from '../../../../src/features/create/QuietPill';
import { superficie } from '../../../../src/theme/superficie';
import * as m from '../../../../src/paraglide/messages';

const one = (v: string | string[] | undefined) => (Array.isArray(v) ? v[0] : v) ?? '';
const noop = () => {};
// Id de menu da conta própria do Claude ('' não serve de id de ação no menu nativo).
const OWN_ENGINE = '#own';
// O cabeçalho é o da pilha (vidro, transparente): a conversa começa abaixo dele.
const EDGES: Edge[] = ['bottom', 'left', 'right'];

// Texto que não chegou à sessão vai para o rascunho dela, que o Composer restaura ao abrir.
// `merge` junta ao que já estava lá (conversa já aberta noutra sessão) em vez de sobrescrever.
function handOffDraft(serverId: string, name: string, text: string, merge: boolean) {
  let prev: ConversationDraft | null = null;
  try { prev = readDraft(serverId, name); } catch { /* ilegível: grava por cima */ }
  const value: ConversationDraft = merge && prev
    ? { ...prev, text: prev.text ? `${prev.text}\n\n${text}` : text, revision: prev.revision + 1 }
    : { version: 1, text, revision: (prev?.revision ?? 0) + 1, transcript: null, attachments: [], submission: null };
  try { writeDraft(serverId, name, value); } catch (e) {
    toast.erro(e instanceof Error ? e.message : m.draft_write_error());
  }
}

// Conversa fechada: histórico só de leitura; enviar retoma a conversa (resume na conta dela) e
// entrega o texto na sessão nova.
export default function ArchivedConversation() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const header = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const params = useLocalSearchParams<{ server: string; project: string; sid: string }>();
  const serverId = one(params.server);
  const project = one(params.project);
  const sid = one(params.sid);
  const ready = useServers((s) => s.ready);
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);

  const [entry, setEntry] = useState<ArchiveEntry | null>(null);
  const [events, setEvents] = useState<ChatEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [draft, setDraft] = useState('');
  const [resuming, setResuming] = useState(false);
  const [resumeError, setResumeError] = useState('');
  // A pasta da conversa sumiu: a folha pergunta onde retomar. Ref: a escolha e o fim dela vêm no mesmo toque.
  const [folderGone, setFolderGone] = useState(false);
  const chosenFolder = useRef('');
  // O pane original morreu: o app não sabe qual motor rodava a conversa. '' = conta Anthropic.
  const [engine, setEngine] = useState('');
  const [motores, setMotores] = useState<Record<string, Motor>>({});
  const [configs, setConfigs] = useState<ConfigDirInfo[]>([]);
  const [codexAccounts, setCodexAccounts] = useState<CodexAccount[]>([]);
  const [cotas, setCotas] = useState<CotaContaResumo[]>([]);
  const [accountsLoading, setAccountsLoading] = useState(false);
  const [accountsError, setAccountsError] = useState('');
  // null = não mexeu: vale a conta dona da conversa.
  const [pickedConfig, setPickedConfig] = useState<string | null>(null);
  const gen = useRef(0);
  const taken = useRef<ArchiveEntry | null>(null);
  // Lido na hora: o objeto do servidor muda de identidade (rede local aprendida) sem ser outra máquina.
  const serverRef = useRef(server);
  serverRef.current = server;
  const hasServer = !!server;

  const open = useCallback(async (e: ArchiveEntry) => {
    const server = serverRef.current;
    if (!server) return;
    const g = ++gen.current;
    setEntry(e);
    setLoading(true);
    setError('');
    setEvents([]);
    setResumeError('');
    setFolderGone(false);
    setEngine('');
    setMotores({});
    setPickedConfig(null);
    setConfigs([]);
    setCodexAccounts([]);
    setCotas([]);
    setAccountsError('');
    if (e.provider === 'claude' || e.provider === 'codex') {
      setAccountsLoading(true);
      const accounts: Promise<ConfigDirInfo[] | CodexAccount[]> = e.provider === 'claude'
        ? listClaudeConfigsForServer(server) : getCodexAccountsForServer(server);
      accounts
        .then((list) => {
          if (g !== gen.current) return;
          if (e.provider === 'claude') setConfigs(list as ConfigDirInfo[]);
          else setCodexAccounts(list as CodexAccount[]);
        })
        // Sem lista não há escolha e retomar segue na conta de origem; o aviso explica o "Padrão" travado.
        .catch((err: unknown) => {
          console.warn('archive: accounts list failed', e.provider, err);
          if (g === gen.current) setAccountsError(m.native_accounts_failed({ reason: err instanceof Error ? err.message : String(err) }));
        })
        .finally(() => { if (g === gen.current) setAccountsLoading(false); });
    } else setAccountsLoading(false);
    if (e.provider === 'claude') {
      // Best-effort: sem motores não há seletor; sem cota não há trava. Retomar continua funcionando.
      getEnginesForServer(server)
        .then((r) => { if (g === gen.current) setMotores(r.motores); })
        .catch((err: unknown) => console.warn('archive: engines list failed', err));
      probeServerResponse(server, '/api/cotas')
        .then((r) => (r.ok ? (r.json() as Promise<CotaContaResumo[]>) : []))
        .then((cs) => { if (g === gen.current) setCotas(cs); })
        .catch((err: unknown) => console.warn('archive: quota read failed', err));
    }
    try {
      const history = await getArchiveHistory(e.project, e.session_id, undefined, e.config_dir, e.provider, e.codex_account, server);
      if (g === gen.current) setEvents(history);
    } catch {
      if (g === gen.current) setError(m.arquivo_conversa_erro());
    } finally {
      if (g === gen.current) setLoading(false);
    }
  }, []);

  // Pela lista de conversas a entrada já vem pronta; sem ela (rota aberta de outro jeito) busca na
  // pasta do servidor dono.
  const resolve = useCallback(() => {
    if (!serverRef.current) return;
    taken.current ??= takeEntry(serverId, project, sid) ?? null;
    if (taken.current) { void open(taken.current); return; }
    const g = ++gen.current;
    setLoading(true);
    setError('');
    // getArchiveFolder fala com o servidor ativo.
    useServers.getState().ensureActive(serverId);
    getArchiveFolder(project)
      .then((items) => {
        if (g !== gen.current) return;
        const matches = items.filter((item) => item.session_id === sid);
        if (matches.length === 1) {
          taken.current = matches[0];
          void open(matches[0]);
          return;
        }
        setError(matches.length > 1 ? m.arquivo_conversa_conta_ambigua() : m.arquivo_conversa_erro());
        setLoading(false);
      })
      .catch(() => {
        if (g !== gen.current) return;
        setError(m.arquivo_conversa_erro());
        setLoading(false);
      });
  }, [serverId, project, sid, open]);

  useEffect(() => {
    if (!ready) return;
    if (!hasServer) {
      setError(m.servidor_nao_existe());
      setLoading(false);
      return;
    }
    resolve();
    return () => { gen.current++; };
  }, [ready, hasServer, resolve]);

  const selectedAccount = entry?.provider === 'codex'
    ? (entry.codex_account ?? null)
    : (pickedConfig ?? entry?.config_dir ?? configs.find((c) => c.active)?.path ?? null);
  // Motor próprio não gasta a conta Claude: a trava não vale.
  const janela = entry?.provider === 'claude' && !engine && selectedAccount
    ? janelaEsgotada(cotaDaConta(cotas, selectedAccount)) : null;
  const blockedNow = janela ? m.cota_conta_no_limite({ janela }) : null;
  const canSend = !!entry && !!draft.trim() && !resuming && !accountsLoading && !blockedNow;

  const sendAndResume = async (cwd?: string) => {
    const text = draft.trim();
    if (!entry || !server || !canSend) return;
    const e = entry;
    const srv = server;
    const g = gen.current;
    setResuming(true);
    setResumeError('');
    let name = '';
    try {
      const info = await resumeArchivedConversation(e.project, e.session_id, engine || null,
        pickedConfig ?? e.config_dir, e.provider, e.codex_account, srv, cwd);
      name = info.name;
      await sendInputForServer(srv, name, text);
      setDraft('');
    } catch (err) {
      if (!name) {
        if (g !== gen.current) return;
        // Conversa já aberta noutra sessão: o texto vai pro rascunho dela e o chat abre lá.
        const x = err as { status?: number; code?: string; envelope?: { sessao?: unknown; params?: { sessao?: unknown } } };
        const live = x.envelope?.sessao ?? x.envelope?.params?.sessao;
        const viva = x.status === 409 && x.code === 'erro_conversa_viva';
        if (viva && typeof live === 'string' && live) {
          handOffDraft(srv.id, live, text, true);
          router.replace(`/s/${srv.id}/${live}` as never);
          return;
        }
        setResumeError(viva ? m.conversa_ja_aberta_sem_nome() : err instanceof Error ? err.message : m.arquivo_retomar_erro());
        if (x.code === 'erro_cwd_inexistente') setFolderGone(true);
        return;
      }
      console.error('archive: resume ok, send failed', name, err);
      handOffDraft(srv.id, name, text, false);
    } finally {
      setResuming(false);
    }
    // Quem saiu da tela durante o resume não é levado para a sessão nova.
    if (name && g === gen.current) router.replace(`/s/${srv.id}/${name}` as never);
  };

  const title = entry ? (entry.preview || entry.ultima || entry.session_id.slice(0, 8)) : sid.slice(0, 8);
  const pasta = entry?.cwd ? basename(entry.cwd) : '';
  const destino = [pasta, server?.label].filter(Boolean).join(' @ ');
  const muted = theme.tokens.text.muted;
  const provider = entry?.provider;
  const textColor = theme.tokens.text.primary;
  const canGoBack = router.canGoBack();
  // Memo: digitar no rascunho redesenha a tela, e opções novas reconfiguram o cabeçalho nativo.
  const headerOptions = useMemo(() => ({
    ...header,
    headerLeft: canGoBack ? undefined : () => <HeaderBack onPress={() => router.replace('/')} />,
    headerTitle: () => (
      <View style={styles.titleRow}>
        {provider ? <ProviderGlyph provider={provider} size={18} /> : <Icon name="MessageCircle" size={18} color={muted} />}
        <View style={styles.titles}>
          <Text style={[styles.title, { color: textColor }]} numberOfLines={1} accessibilityRole="header">{title}</Text>
          {destino ? <Text style={[styles.destino, { color: muted }]} numberOfLines={1}>{destino}</Text> : null}
        </View>
      </View>
    ),
  }), [header, canGoBack, router, provider, muted, textColor, title, destino]);

  let body;
  if (loading) {
    body = (
      <View style={styles.center}>
        <ActivityIndicator />
        <Text style={styles.hint}>{m.arquivo_carregando()}</Text>
      </View>
    );
  } else if (error) {
    body = (
      <View style={styles.center}>
        <Text style={styles.hint} accessibilityRole="alert">{error}</Text>
        {server ? (
          <Text style={styles.retry} onPress={() => (entry ? void open(entry) : resolve())} accessibilityRole="button">
            {m.lista_tentar_novamente()}
          </Text>
        ) : null}
      </View>
    );
  } else if (events.length === 0) {
    body = <View style={styles.center}><Text style={styles.hint}>{m.arquivo_sem_mensagens()}</Text></View>;
  } else {
    body = <MessageList events={events} preview="" olderFailed="" onLoadOlder={noop} />;
  }

  const claudeLabel = configs.find((c) => c.path === selectedAccount)?.label ?? m.criar_padrao();
  const codexLabel = codexAccounts.find((a) => a.id === selectedAccount)?.name ?? m.criar_padrao();
  const note = blockedNow ?? (resumeError || accountsError);

  return (
    <Screen edges={EDGES}>
      <Stack.Screen options={headerOptions} />
      <KeyboardAvoidingView behavior="padding" automaticOffset style={[styles.flex, { paddingTop: headerHeight }]}>
        <View style={styles.flex}>{body}</View>
        {entry && !error ? (
          <View style={styles.dock}>
            {note ? <Text style={[styles.note, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{note}</Text> : null}
            <Glass variant="chrome" style={styles.box}>
              <View style={styles.inputRow}>
                <View style={styles.flex}>
                  <MultilineInput
                    value={draft}
                    onChangeText={setDraft}
                    placeholder={m.conversa_continuar_placeholder()}
                    accessibilityLabel={m.conversa_continuar_placeholder()}
                    editable={!resuming}
                    maxHeight={160}
                  />
                </View>
                <Pressable
                  onPress={() => void sendAndResume()}
                  disabled={!canSend}
                  hitSlop={5}
                  accessibilityRole="button"
                  accessibilityLabel={m.nova_conversa_enviar()}
                  accessibilityState={{ disabled: !canSend, busy: resuming }}
                  style={[styles.send, { backgroundColor: canSend ? theme.tokens.text.primary : superficie(theme, 0.8) }]}
                >
                  {resuming
                    ? <ActivityIndicator size="small" color={muted} />
                    : <Icon name="ArrowUp" size={18} color={canSend ? theme.tokens.bg.base : muted} />}
                </Pressable>
              </View>
            </Glass>
            <View style={styles.pills}>
              {/* Motor é do Claude: mandá-lo num resume de outro provider exportaria chave pra um CLI que não a lê. */}
              {entry.provider === 'claude' && Object.keys(motores).length ? (
                <QuietPill
                  icon="Cpu"
                  label={engine ? (motores[engine]?.label ?? engine) : m.criar_claude_sua_conta()}
                  aria={m.comum_motor()}
                  menuTitle={m.arquivo_motor_escolha()}
                  disabled={resuming}
                  actions={[
                    { id: OWN_ENGINE, title: m.criar_claude_sua_conta(), state: engine ? 'off' : 'on' },
                    ...Object.entries(motores).map(([nome, motor]) => ({
                      id: nome, title: motor.label ?? nome, subtitle: motor.model, state: (nome === engine ? 'on' : 'off') as 'on' | 'off',
                    })),
                  ]}
                  onAction={(id) => setEngine(id === OWN_ENGINE ? '' : id)}
                />
              ) : null}
              {entry.provider === 'claude' ? (
                <QuietPill
                  icon={janela ? 'TriangleAlert' : 'CircleUser'}
                  tone={janela ? 'warning' : undefined}
                  label={accountsLoading ? m.comum_carregando() : claudeLabel}
                  aria={m.comum_conta_claude()}
                  menuTitle={m.comum_conta_claude()}
                  disabled={resuming || accountsLoading || !configs.length}
                  actions={configs.map((c) => ({ id: c.path, title: c.label, state: (c.path === selectedAccount ? 'on' : 'off') as 'on' | 'off' }))}
                  onAction={setPickedConfig}
                />
              ) : entry.provider === 'codex' ? (
                // A conta do Codex é a dona do transcript: retomar noutra não acharia a conversa.
                <QuietPill
                  icon="CircleUser"
                  label={accountsLoading ? m.comum_carregando() : codexLabel}
                  aria={m.native_create_codex_account()}
                  disabled
                />
              ) : null}
            </View>
          </View>
        ) : null}
      </KeyboardAvoidingView>
      {folderGone && server ? (
        <Sheet open sizes={['large']} scrollable onDismiss={() => setFolderGone(false)}>
          <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.folderSheet}>
            <Text style={[styles.folderTitle, { color: textColor }]} accessibilityRole="header">{m.retomar_escolher_pasta()}</Text>
            <Text style={[styles.folderNote, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{resumeError}</Text>
            <CwdPicker
              server={server}
              autoSelect={false}
              onPick={(path) => { chosenFolder.current = path; }}
              onDone={() => {
                setFolderGone(false);
                void sendAndResume(chosenFolder.current);
              }}
            />
          </KeyboardAvoidingView>
        </Sheet>
      ) : null}
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  flex: { flex: 1 },
  titleRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], flexShrink: 1 },
  titles: { flexShrink: 1, minWidth: 0 },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  destino: { fontSize: theme.base.text.xxs },
  center: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: theme.base.space[2], padding: theme.base.space[6] },
  hint: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
  retry: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
    color: theme.tokens.accent.base,
    minHeight: 40,
    lineHeight: 38,
    paddingHorizontal: theme.base.space[4],
    borderWidth: 1,
    borderColor: theme.tokens.accent.base,
    borderRadius: theme.base.radius.full,
    overflow: 'hidden',
  },
  folderSheet: { flex: 1, minHeight: 480, paddingHorizontal: theme.base.space[4], gap: theme.base.space[2] },
  folderTitle: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  folderNote: { fontSize: theme.base.text.sm },
  dock: { paddingTop: theme.base.space[2], gap: theme.base.space[1] },
  note: { fontSize: theme.base.text.sm, paddingHorizontal: theme.base.space[4] },
  box: {
    marginHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.lg,
    paddingVertical: theme.base.space[2],
    paddingLeft: theme.base.space[3],
    paddingRight: theme.base.space[2],
  },
  inputRow: { flexDirection: 'row', alignItems: 'flex-end', gap: theme.base.space[1] },
  send: { width: 34, height: 34, marginBottom: theme.base.space[0.5], borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
  pills: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[0.5],
    paddingHorizontal: theme.base.space[3],
    paddingTop: theme.base.space[1],
    paddingBottom: theme.base.space[2],
    minHeight: 34,
  },
}));
