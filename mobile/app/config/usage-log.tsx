import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Platform, ScrollView, Switch, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { Directory } from 'expo-file-system';
import * as Clipboard from 'expo-clipboard';
import { runtimeVersion } from 'expo-updates';
import {
  DIAG_REPORT_TEXT_MAX, getDiagFileForServer, getDiagSummaryForServer, sendDiagReportAll,
  type DiagReportFailureKind, type LinhaDiag, type ResumoDiag, type Server,
} from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { SettingsRow } from '../../src/features/config/SettingsRow';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { useSettingsColors } from '../../src/features/config/colors';
import { useInputStyle } from '../../src/features/config/ServerConfigParts';
import { Segmented } from '../../src/features/config/Segmented';
import { ownServers, useServers } from '../../src/stores/servers';
import { haptic } from '../../src/ui/haptics';
import { toast } from '../../src/ui/Toast';
import * as m from '../../src/paraglide/messages';

/** `missing`: servidor anterior à rota do diário; não é falha, só não há o que mostrar. */
type Diary = ResumoDiag & { missing: boolean };
type Download = { state: 'choosing' | 'downloading' } | { state: 'done'; text: string } | { state: 'failed'; text: string };

function sizeText(bytes: number): string {
  const [value, unit] = bytes >= 1 << 20 ? [bytes / (1 << 20), 'MB'] : bytes >= 1024 ? [bytes / 1024, 'KB'] : [bytes, 'B'];
  return `${unit === 'B' ? value.toFixed(0) : value.toFixed(1).replace('.', m.native_decimal())} ${unit}`;
}

/** "2026-09-24T14:37:12-03:00" → "24/09 14:37:12", no relógio de quem gravou. */
function when(ts: string): string {
  const match = /^\d{4}-(\d{2})-(\d{2})T(\d{2}:\d{2}:\d{2})/.exec(ts ?? '');
  return match ? `${match[2]}/${match[1]} ${match[3]}` : '--/-- --:--:--';
}

// O código HTTP só quando não deu certo, como no desktop.
function context(line: LinhaDiag): string {
  const platform = line.so ? [line.so, line.navegador, line.vista, line.tela_px].filter(Boolean).join(' · ') : '';
  const code = line.codigo && line.nivel && line.nivel !== 'ok' ? `#${line.codigo}` : '';
  return [line.detalhe, platform, line.sessao, line.tela, code, line.ms != null ? `${line.ms}ms` : '']
    .filter(Boolean).join(' · ');
}

type Failure = { code: string; kind: DiagReportFailureKind };
type Missed = { server: Server; failure: Failure };
type Report = { state: 'sending' } | { state: 'done'; code: string | null; diary: 'sent' | 'failed'; missed: Missed[] };
type Scope = 'all' | 'this' | 'pick';

const CLIENT = `expo ${runtimeVersion ?? 'dev'} ${Platform.OS}`;

function failureText({ code, kind }: Failure): string {
  if (kind === 'unsupported') return m.native_settings_diary_report_unsupported();
  if (kind === 'rate_limited') return m.native_settings_diary_report_rate_limited();
  if (kind === 'unreachable') return m.native_settings_diary_report_unreachable();
  return m.native_settings_diary_report_failed({ code });
}

/** Relatório de diagnóstico: por padrão de todas as máquinas da pessoa (o erro costuma estar noutra, não na
 *  da tela). Convidado não vê a seção: o servidor recusaria. */
function ReportSection({ server }: { server: Server }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const input = useInputStyle();
  const all = useServers((s) => s.servers);
  const machines = useMemo(() => ownServers(all), [all]);
  const [scope, setScope] = useState<Scope>('all');
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const [text, setText] = useState('');
  const [report, setReport] = useState<Report | null>(null);
  const [copied, setCopied] = useState(false);
  const generation = useRef(0);

  useEffect(() => {
    setReport(null);
    setCopied(false);
    return () => { generation.current++; };
  }, [server.id, server.baseUrl, server.token]);

  const many = machines.length > 1;
  const targets = !many || scope === 'this' ? [server]
    : scope === 'all' ? machines : machines.filter((s) => picked[s.id] ?? s.id === server.id);

  const send = async () => {
    if (report?.state === 'sending' || !targets.length) return;
    const now = ++generation.current;
    setReport({ state: 'sending' });
    setCopied(false);
    const result = await sendDiagReportAll(targets, { text, client: CLIENT });
    if (now !== generation.current) return;
    const missed = result.results.flatMap(({ server: s, result: r }) => r.ok ? [] : [{ server: s, failure: { code: r.code, kind: r.kind } }]);
    const diary = result.results.some(({ result: r }) => r.ok && r.diary === 'failed') ? 'failed' : 'sent';
    if (result.code) {
      haptic.success();
      setText('');
    } else if (missed[0]) toast.erro(failureText(missed[0].failure));
    setReport({ state: 'done', code: result.code, diary, missed });
  };

  const copy = async (code: string) => {
    try {
      await Clipboard.setStringAsync(code);
      haptic.success();
      setCopied(true);
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : m.erro_desconhecido());
    }
  };

  const sending = report?.state === 'sending';
  const sentCode = report?.state === 'done' ? report.code : null;
  const diaryFailed = report?.state === 'done' && report.diary === 'failed';
  const missed = report?.state === 'done' ? report.missed : [];
  const unsupported = report?.state === 'done' && !report.code && missed.every((x) => x.failure.kind === 'unsupported');
  return (
    <SectionCard icon="LifeBuoy" title={m.native_settings_diary_report_title()}>
      {many ? (
        <SettingsRow icon="Server" title={m.native_settings_diary_report_scope_label()} description={m.native_settings_diary_report_server_many()}>
          <Segmented<Scope>
            options={[
              { v: 'all', label: m.native_settings_diary_report_scope_all({ count: machines.length }), aria: m.native_settings_diary_report_scope_all_long({ count: machines.length }) },
              { v: 'this', label: m.native_settings_diary_report_scope_this(), aria: m.native_settings_diary_report_scope_this_long({ server: server.label }) },
              { v: 'pick', label: m.native_settings_diary_report_scope_pick() },
            ]}
            value={scope}
            onChange={setScope}
            label={m.native_settings_diary_report_scope_label()}
            disabled={sending}
          />
        </SettingsRow>
      ) : (
        <SettingsRow icon="Server" title={server.label} description={m.native_settings_diary_report_server({ server: server.label })} />
      )}
      {many && scope === 'pick' ? machines.map((s) => (
        <SettingsRow key={s.id} icon="Monitor" title={s.label}
          right={<Switch value={picked[s.id] ?? s.id === server.id} disabled={sending} accessibilityLabel={s.label}
            onValueChange={(on) => setPicked((p) => ({ ...p, [s.id]: on }))} />} />
      )) : null}
      <SettingsRow icon="MessageSquare" title={m.native_settings_diary_report_text_label()}>
        <TextInput
          {...input}
          style={[input.style, styles.textArea]}
          value={text}
          onChangeText={setText}
          placeholder={m.native_settings_diary_report_text_placeholder()}
          accessibilityLabel={m.native_settings_diary_report_text_label()}
          maxLength={DIAG_REPORT_TEXT_MAX}
          multiline
          textAlignVertical="top"
          editable={!sending}
        />
      </SettingsRow>
      <SettingsRow icon="ShieldCheck" title={m.native_settings_diary_report_send()} description={m.native_settings_diary_report_notice()}>
        {sentCode ? (
          <View style={styles.sent}>
            <Text accessibilityLiveRegion="polite" style={[styles.noteText, { color: c.text }]}>{m.native_settings_diary_report_sent()}</Text>
            <Text selectable style={[styles.code, { color: c.text, fontFamily: theme.base.fontMono }]}>{sentCode}</Text>
            {diaryFailed ? (
              <Text style={[styles.noteText, { color: theme.tokens.status.warning }]}>{m.native_settings_diary_report_diary_failed()}</Text>
            ) : null}
          </View>
        ) : null}
        {missed.length ? (
          <View accessibilityRole="alert" style={styles.sent}>
            {targets.length > 1 ? (
              <Text style={[styles.noteText, { color: theme.tokens.status.error }]}>{m.native_settings_diary_report_machines_failed()}</Text>
            ) : null}
            {missed.map((x) => (
              <Text key={x.server.id} style={[styles.noteText, { color: theme.tokens.status.error }]}>
                {targets.length > 1 ? m.native_settings_diary_report_machine_failed({ server: x.server.label, reason: failureText(x.failure) }) : failureText(x.failure)}
              </Text>
            ))}
          </View>
        ) : null}
        <View style={styles.row}>
          {sentCode ? (
            <Pill
              icon={copied ? 'Check' : 'Copy'}
              label={copied ? m.native_settings_diary_report_copied() : m.native_settings_diary_report_copy()}
              onPress={() => { void copy(sentCode); }}
            />
          ) : (
            <Pill
              icon={report?.state === 'done' ? 'RefreshCw' : 'Send'}
              label={sending ? m.native_settings_diary_report_sending()
                : report?.state === 'done' ? m.native_settings_diary_report_retry()
                : m.native_settings_diary_report_send()}
              disabled={sending || unsupported || !targets.length}
              onPress={() => { void send(); }}
            />
          )}
        </View>
      </SettingsRow>
    </SectionCard>
  );
}

const stamp = () => new Date().toISOString().slice(0, 16).replace(/[-:]/g, '').replace('T', '-');
const cancelled = (e: unknown) => /CANCEL/i.test(String((e as { code?: string }).code ?? ''));

export default function UsageLogPage() {
  const server = useServers((s) => s.active());
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [diary, setDiary] = useState<Diary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [download, setDownload] = useState<Download | null>(null);
  const generation = useRef(0);

  const load = useCallback(() => {
    const now = ++generation.current;
    if (!server) return;
    setLoading(true);
    setError('');
    // O "Salvo em…" anterior sai: a linha volta a descrever o diário que está chegando.
    setDownload((d) => (d?.state === 'choosing' || d?.state === 'downloading' ? d : null));
    getDiagSummaryForServer(server)
      .then((next) => { if (now === generation.current) setDiary({ ...next, missing: false }); })
      .catch((cause: unknown) => {
        if (now !== generation.current) return;
        if ((cause as { status?: number }).status === 404) {
          setDiary({ dias: 0, bytes: 0, arquivos: [], dias_guardados: 7, ultimas: [], missing: true });
        } else setError(cause instanceof Error ? cause.message : m.native_invalid_response());
      })
      .finally(() => { if (now === generation.current) setLoading(false); });
  }, [server]);

  useEffect(() => {
    setDiary(null);
    setDownload(null);
    load();
    return () => { generation.current++; };
  }, [load, server?.id, server?.baseUrl, server?.token]);

  // Escolhe onde salvar primeiro; só então pede o arquivo ao servidor.
  const save = async () => {
    if (!server || download?.state === 'choosing' || download?.state === 'downloading') return;
    setDownload({ state: 'choosing' });
    let folder: Directory;
    try {
      folder = await Directory.pickDirectoryAsync();
    } catch (e) {
      setDownload(cancelled(e) ? null : { state: 'failed', text: m.native_save_dialog_failed() });
      return;
    }
    setDownload({ state: 'downloading' });
    try {
      const text = await getDiagFileForServer(server);
      const name = `hangar-uso-${stamp()}.jsonl`;
      try {
        folder.createFile(name, 'application/x-ndjson').write(text);
      } catch {
        setDownload({ state: 'failed', text: m.native_save_failed() });
        return;
      }
      setDownload({ state: 'done', text: m.native_saved({ path: `${folder.name}/${name}` }) });
    } catch (cause) {
      setDownload({ state: 'failed', text: cause instanceof Error ? cause.message : m.native_save_failed() });
    }
  };

  const header = (
    <PageHeader
      title={m.native_settings_page_diary()}
      subtitle={m.native_settings_diary_lead({ server: server?.label ?? '' })}
    />
  );
  if (!server) return <Pagina>{header}<InfoNotice text={m.native_settings_offline()} /></Pagina>;

  const keep = diary?.dias_guardados ?? 7;
  const status = download?.state === 'choosing' ? m.native_settings_diary_choosing()
    : download?.state === 'downloading' ? m.native_settings_diary_downloading()
    : download && 'text' in download ? download.text
    : diary && diary.dias > 0 ? m.native_settings_diary_has({ days: String(diary.dias), size: sizeText(diary.bytes) })
    : diary ? (diary.missing ? m.native_diary_missing() : m.native_settings_diary_empty())
    : loading ? m.native_settings_diary_loading() : '';
  const busy = download?.state === 'choosing' || download?.state === 'downloading';
  const levelColor = (l: LinhaDiag['nivel']) =>
    l === 'erro' ? theme.tokens.status.error : l === 'aviso' ? theme.tokens.status.warning : c.text;
  const note = (text: string, color: string, alert = false) => (
    <Text accessibilityRole={alert ? 'alert' : undefined} style={[styles.note, { color }]}>{text}</Text>
  );

  return (
    <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.flex}>
    <Pagina>
      {header}
      <SectionCard icon="ShieldCheck" title={m.native_settings_diary_rules()}>
        <SettingsRow icon="HardDrive" title={m.native_settings_diary_rule_local()} />
        <SettingsRow icon="EyeOff" title={m.native_settings_diary_rule_private()} />
        <SettingsRow icon="Clock" title={m.native_settings_diary_rule_keep({ days: String(keep) })} />
      </SectionCard>
      <SectionCard icon="FileText" title={m.native_settings_diary_file()}>
        <SettingsRow icon="Download" title={m.native_settings_diary_download()} description={download?.state === 'failed' ? undefined : status}>
          {download?.state === 'failed' ? note(download.text, theme.tokens.status.error, true) : null}
          {/* Sem dias gravados não há o que baixar: o botão some em vez de baixar um arquivo vazio. */}
          {diary && diary.dias > 0 && !busy ? (
            <View style={styles.row}><Pill icon="Download" label={m.native_settings_diary_download()} onPress={() => { void save(); }} /></View>
          ) : null}
        </SettingsRow>
      </SectionCard>
      {server.invite ? null : <ReportSection server={server} />}
      <SectionCard
        icon="List"
        title={m.native_settings_diary_recent()}
        extra={<Pill icon="RefreshCw" label={loading ? m.native_settings_diary_reloading() : m.native_settings_diary_reload()} onPress={() => { if (!loading) load(); }} />}
      >
        {diary && diary.ultimas.length ? (
          <ScrollView style={styles.lines} nestedScrollEnabled>
            {diary.ultimas.map((line, n) => (
              <View key={`${line.ts}-${n}`} style={[styles.line, { borderTopColor: c.border }]}>
                <View style={styles.lineHead}>
                  <Text style={[styles.mono, { color: c.muted }]}>{when(line.ts)}</Text>
                  <Text style={[styles.mono, styles.event, { color: levelColor(line.nivel) }]} numberOfLines={1}>{line.evento || '?'}</Text>
                </View>
                {context(line) ? <Text style={[styles.mono, { color: c.muted }]}>{context(line)}</Text> : null}
              </View>
            ))}
          </ScrollView>
        ) : diary ? note(diary.missing ? m.native_diary_missing() : m.native_settings_diary_empty(), c.muted)
          : error && !loading ? note(error, theme.tokens.status.error, true)
          : note(m.native_settings_diary_loading(), c.muted)}
        {diary && error ? note(error, theme.tokens.status.error, true) : null}
      </SectionCard>
    </Pagina>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create((theme) => ({
  flex: { flex: 1 },
  textArea: { minHeight: 88, paddingTop: theme.base.space[2], paddingBottom: theme.base.space[2] },
  sent: { gap: theme.base.space[1.5] },
  noteText: { fontSize: theme.base.text.xs },
  code: { fontSize: theme.base.text.xl, fontWeight: theme.base.weight.semibold, letterSpacing: 2 },
  row: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
  note: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], fontSize: theme.base.text.xs },
  lines: { maxHeight: 420 },
  line: { borderTopWidth: 1, paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[2], gap: theme.base.space[0.5] },
  lineHead: { flexDirection: 'row', gap: theme.base.space[2], alignItems: 'baseline' },
  event: { flexShrink: 1, fontWeight: theme.base.weight.semibold },
  mono: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs },
}));
