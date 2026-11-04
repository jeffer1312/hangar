import { useEffect, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Switch, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { basename, deleteWorktreeForServer, fmtBytes, getWorktreeForServer, intlLocale, relativeTime, worktreeState,
         type Server, type WorktreeState, type WorktreeStatus } from '@hangar/core';
import * as Clipboard from 'expo-clipboard';
import { Sheet } from '../../ui/Sheet';
import { Chip } from '../../ui/Chip';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { StateDot } from '../../ui/StateDot';
import * as m from '../../paraglide/messages';
import { dropWorktreeStatus, putWorktreeStatus } from './worktreeStatus';
import { displayTitle, openSession, sessionStateLabel, sizeLabel, STATE_TONE, stateLabel, useLiveSessionState } from './worktreeUi';

type Props = { server: Server | null; path: string | null; onClose: () => void; onDeleted?: () => void };

async function copiar(texto: string) {
  try {
    await Clipboard.setStringAsync(texto);
    toast.ok(m.toast_copiado());
  } catch (e) {
    toast.erro(e instanceof Error ? e.message : String(e));
  }
}

const dataHora =(at: number | null | undefined) =>
  at ? new Date(at * 1000).toLocaleString(intlLocale(), { dateStyle: 'medium', timeStyle: 'short' }) : '—';

function veredito(k: WorktreeState, st: WorktreeStatus): string {
  switch (k) {
    case 'gone': return m.worktree_veredito_sumida();
    case 'session': return m.worktree_veredito_em_uso();
    case 'residual': return m.worktree_residual_warning();
    case 'dirty': return m.worktree_veredito_nao_commitado({ n: st.dirty });
    case 'merged': return m.worktree_veredito_mesclada();
    case 'detached': return m.worktree_veredito_sem_branch();
    case 'active': return m.worktree_veredito_andamento({ n: st.ahead });
  }
}

export function WorktreeSheet({ server, path, onClose, onDeleted }: Props) {
  const [lido, setSt] = useState<WorktreeStatus | null>(null);
  // Reabrir em outra worktree nunca mostra os dados nem o botão de apagar da anterior.
  const st = lido && lido.path === path ? lido : null;
  const [erro, setErro] = useState('');
  const [apagando, setApagando] = useState(false);
  const [apagarBranch, setApagarBranch] = useState(false);
  const [confirmando, setConfirmando] = useState(false);
  const serverId = server?.id;

  // Chave pelo id: a lista de servidores troca de objeto a cada atualização do store, e isso não
  // pode zerar a folha aberta.
  useEffect(() => {
    if (!server || !path) { setSt(null); return; }
    let vivo = true;
    setSt(null); setErro(''); setApagarBranch(false); setConfirmando(false);
    getWorktreeForServer(server, path)
      .then((r) => { if (vivo) { setSt(r); putWorktreeStatus(server.id, r); } })
      .catch((e: unknown) => { if (vivo) setErro(e instanceof Error ? e.message : String(e)); });
    return () => { vivo = false; };
  }, [serverId, path]);

  const nPerde = st ? st.dirty + st.ignored.length : 0;

  async function apagar() {
    if (!server || !st) return;
    setApagando(true); setErro('');
    try {
      await deleteWorktreeForServer(server, { repo: st.repo, path: st.path, confirm: !!st.residual || nPerde > 0,
        delete_branch: !st.residual && apagarBranch, ...(st.residual ? { keep_branch: true } : {}) });
      dropWorktreeStatus(server.id, st.path);
      onDeleted?.();
      onClose();
    } catch (e) {
      setErro(e instanceof Error ? e.message : String(e));
    } finally {
      setApagando(false);
    }
  }

  const k = st ? worktreeState(st) : null;
  return (
    <Sheet open={!!path} sizes={['medium', 'large']} onDismiss={onClose} scrollable>
      <ScrollView contentContainerStyle={styles.inner} nestedScrollEnabled>
        {!st ? <Text style={styles.title}>{path ? basename(path) : ''}</Text> : null}
        {!st && !erro ? <Text style={styles.muted} accessibilityLiveRegion="polite">{m.comum_carregando()}</Text> : null}
        {!st && erro ? <Text accessibilityRole="alert" style={styles.erro}>{m.worktrees_erro({ motivo: erro })}</Text> : null}
        {st && k && !confirmando ? (
          <>
            <View style={styles.cabeca}>
              <Text style={[styles.title, styles.flex]} accessibilityRole="header">{displayTitle(st)}</Text>
              <Chip tone={STATE_TONE[k]}>{stateLabel(k)}</Chip>
            </View>
            <View style={styles.linha}>
              <Text style={[styles.mono, styles.flex]} selectable>{st.path}</Text>
              <Pressable onPress={() => void copiar(st.path)} accessibilityRole="button" accessibilityLabel={m.worktree_copiar_caminho()}
                hitSlop={8} style={styles.copiar}>
                <Icon name="Copy" size={16} />
              </Pressable>
            </View>
            <Text style={styles.text}>{veredito(k, st)}</Text>
            {st.residual && st.residual_entries == null ? <Text style={styles.aviso}>{m.worktree_residual_inventory_incomplete()}</Text> : null}
            {st.degraded && !st.residual ? <Text style={styles.aviso}>{m.worktree_leitura_incompleta()}</Text> : null}

            {st.sessions.length && server ? (
              <Secao titulo={m.worktree_sessoes_aqui()}>
                {st.sessions.map((n) => <SessaoAqui key={n} serverId={server.id} name={n} onClose={onClose} />)}
              </Secao>
            ) : null}

            <Secao titulo={m.worktree_detalhe_branch()}>
              <Text style={styles.mono}>{st.base ?? '—'}  <Text style={styles.muted}>{m.worktree_detalhe_base({ n: st.behind ?? 0 })}</Text></Text>
              <Text style={styles.mono}>{st.branch ?? m.worktree_sem_branch_rotulo()}  <Text style={styles.muted}>{m.worktree_detalhe_so_dela({ n: st.ahead })}</Text></Text>
            </Secao>

            <View style={styles.fatos}>
              <Fato rotulo={m.worktree_detalhe_criada()} valor={dataHora(st.created_at)} />
              <Fato rotulo={m.worktree_detalhe_ultimo_commit()} valor={dataHora(st.last_commit?.at)} />
              <Fato rotulo={m.worktree_detalhe_conversas()} valor={String(st.closed)} />
              <Fato rotulo={m.worktree_detalhe_espaco()} valor={sizeLabel(st)} />
              {st.size_biggest ? <Fato rotulo={m.worktree_detalhe_maior()} valor={`${st.size_biggest.name} · ${fmtBytes(st.size_biggest.bytes)}`} /> : null}
            </View>

            <Secao titulo={m.worktree_detalhe_commits()}>
              {st.commits?.length ? st.commits.map((c) => (
                <View key={c.sha} style={styles.commit}>
                  <Text style={styles.sha}>{c.sha.slice(0, 7)}</Text>
                  <Text style={[styles.text, styles.flex]} numberOfLines={1}>{c.subject}</Text>
                  <Text style={styles.muted}>{relativeTime(c.at)}</Text>
                </View>
              )) : <Text style={styles.muted}>{m.worktree_detalhe_sem_commits()}</Text>}
            </Secao>

            {st.dirty ? (
              <Secao titulo={m.worktree_detalhe_nao_commitado({ n: st.dirty })}>
                {st.dirty_files?.map((f) => <Arquivo key={f.path} code={f.code} path={f.path} />)}
              </Secao>
            ) : null}

            {st.sessions.length ? (
              <Text style={styles.aviso}>{m.worktree_bloqueada({ nomes: st.sessions.join(', ') })}</Text>
            ) : (
              <Pressable onPress={() => setConfirmando(true)} accessibilityRole="button" accessibilityLabel={m.worktree_apagar_reticencias()}
                style={styles.botaoContorno}>
                <Text style={styles.erroForte}>{m.worktree_apagar_reticencias()}</Text>
              </Pressable>
            )}
          </>
        ) : null}

        {st && confirmando ? (
          <>
            <Text style={styles.title} accessibilityRole="header">{m.worktree_apagar_titulo({ nome: displayTitle(st) })}</Text>
            <Text style={styles.mono}>{st.path}{st.branch ? ` · ${st.branch}` : ''}</Text>
            {st.size ? <Text style={styles.ok}>{m.worktree_libera({ tamanho: sizeLabel(st) })}</Text> : null}
            {st.residual ? <View style={styles.perdaCaixa}>
              <Text style={styles.aviso}>{m.worktree_residual_warning()}</Text>
              {st.residual_entries == null ? <Text style={styles.aviso}>{m.worktree_residual_inventory_incomplete()}</Text> : null}
              {st.residual_entries != null ? <Text style={styles.text}>{m.worktree_residual_entries({ n: st.residual_entries })}</Text> : null}
              {st.residual_files?.map((file) => <Arquivo key={file} code="" path={file} />)}
            </View> : null}

            {nPerde ? (
              <View style={styles.perdaCaixa}>
                <Text style={styles.erroForte}>{m.worktree_apagar_perde()}</Text>
                {st.dirty ? <Text style={styles.text}>{m.worktree_n_nao_commitados({ n: st.dirty })}</Text> : null}
                {st.dirty_files?.map((f) => <Arquivo key={f.path} code={f.code} path={f.path} />)}
                {st.ignored.length ? <Text style={styles.text}>{m.worktree_ignorados({ nomes: st.ignored.join(', ') })}</Text> : null}
              </View>
            ) : null}

            <Text style={styles.secao}>{m.worktree_fica_guardado()}</Text>
            {st.merged ? (
              <Guardado>{m.worktree_apagar_branch_juntada({ branch: st.branch ?? '', base: st.base ?? '' })}</Guardado>
            ) : st.branch && st.ahead > 0 && !apagarBranch ? (
              <Guardado>{m.worktree_fica_commits({ n: st.ahead, branch: st.branch })}</Guardado>
            ) : null}
            <Guardado>
              {st.closed ? m.worktree_fica_conversas({ n: st.closed }) : m.worktree_apagar_conversas({ branch: st.main_branch ?? st.base ?? '' })}
            </Guardado>

            {!st.residual && !st.merged && st.branch ? (
              <View style={styles.linha}>
                <Switch value={apagarBranch} onValueChange={setApagarBranch} accessibilityLabel={m.worktree_apagar_branch_tambem({ n: st.ahead })} />
                <View style={styles.flex}>
                  <Text style={styles.text}>{m.worktree_apagar_branch_tambem({ n: st.ahead })}</Text>
                  <Text style={styles.muted}>{m.worktree_apagar_branch_perde({ n: st.ahead })}</Text>
                </View>
              </View>
            ) : null}

            {erro ? <Text accessibilityRole="alert" style={styles.erro}>{erro}</Text> : null}
            <View style={styles.acoes}>
              <Pressable onPress={() => { setConfirmando(false); setErro(''); }} disabled={apagando} accessibilityRole="button"
                accessibilityState={{ disabled: apagando }} style={styles.botaoContorno}>
                <Text style={styles.text}>{m.comum_cancelar()}</Text>
              </Pressable>
              <Pressable onPress={apagar} disabled={apagando} accessibilityRole="button"
                accessibilityLabel={nPerde ? m.worktree_apagar_perder({ n: nPerde }) : m.worktree_apagar()}
                accessibilityState={{ busy: apagando, disabled: apagando }} style={styles.botao}>
                <Text style={styles.botaoTexto}>{nPerde ? m.worktree_apagar_perder({ n: nPerde }) : m.worktree_apagar()}</Text>
              </Pressable>
            </View>
          </>
        ) : null}
      </ScrollView>
    </Sheet>
  );
}

function SessaoAqui({ serverId, name, onClose }: { serverId: string; name: string; onClose: () => void }) {
  const state = useLiveSessionState(serverId, name);
  return (
    <View style={styles.sessao}>
      <View style={[styles.linha, styles.flex]}>
        <StateDot state={state ?? 'idle'} />
        <Text style={[styles.text, styles.flex]} numberOfLines={1}>{name}</Text>
        <Text style={styles.muted}>{sessionStateLabel(state)}</Text>
      </View>
      <Pressable onPress={() => { onClose(); openSession(serverId, name); }} accessibilityRole="button"
        accessibilityLabel={m.worktree_ir_sessao_nome({ nome: name })} style={styles.botaoContorno}>
        <Text style={styles.text}>{m.worktree_ir_sessao()}</Text>
      </Pressable>
    </View>
  );
}

function Secao({ titulo, children }: { titulo: string; children: ReactNode }) {
  return (
    <View style={styles.bloco}>
      <Text style={styles.secao}>{titulo}</Text>
      {children}
    </View>
  );
}

function Fato({ rotulo, valor }: { rotulo: string; valor: string }) {
  return (
    <View style={styles.fato}>
      <Text style={styles.muted}>{rotulo}</Text>
      <Text style={[styles.text, styles.fatoValor]} numberOfLines={1}>{valor}</Text>
    </View>
  );
}

function Arquivo({ code, path }: { code: string; path: string }) {
  return (
    <Text style={styles.arquivo} numberOfLines={1}>
      <Text style={styles.codigo}>{code.padEnd(2)} </Text>{path}
    </Text>
  );
}

function Guardado({ children }: { children: string }) {
  return (
    <View style={styles.linha}>
      <Text style={styles.ok}>✓</Text>
      <Text style={[styles.text, styles.flex]}>{children}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { padding: theme.base.space[4], gap: theme.base.space[3] },
  flex: { flex: 1, minWidth: 0 },
  cabeca: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  title: { fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  text: { color: theme.tokens.text.primary },
  muted: { color: theme.tokens.text.secondary },
  mono: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  aviso: { color: theme.tokens.status.warning },
  ok: { color: theme.tokens.status.success },
  erro: { color: theme.tokens.status.error },
  erroForte: { color: theme.tokens.status.error, fontWeight: theme.base.weight.semibold },
  bloco: { gap: theme.base.space[1.5] },
  secao: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.semibold, letterSpacing: 0.6, textTransform: 'uppercase', color: theme.tokens.text.muted },
  fatos: { gap: theme.base.space[1.5] },
  fato: { flexDirection: 'row', justifyContent: 'space-between', gap: theme.base.space[3] },
  fatoValor: { flexShrink: 1, textAlign: 'right' },
  commit: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  sha: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  arquivo: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs, color: theme.tokens.text.secondary },
  codigo: { color: theme.tokens.status.warning },
  sessao: { gap: theme.base.space[2] },
  copiar: { minWidth: 36, minHeight: 36, alignItems: 'center', justifyContent: 'center' },
  perdaCaixa: { gap: theme.base.space[1.5], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderWidth: 1,
                borderColor: theme.tokens.status.error + '55' },
  linha: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  acoes: { flexDirection: 'row', justifyContent: 'flex-end', gap: theme.base.space[2], marginTop: theme.base.space[2] },
  botaoContorno: { alignSelf: 'flex-start', minHeight: 44, paddingHorizontal: theme.base.space[4], justifyContent: 'center',
                   borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  botao: { minHeight: 44, paddingHorizontal: theme.base.space[4], justifyContent: 'center',
           borderRadius: theme.base.radius.md, backgroundColor: theme.tokens.status.error },
  botaoTexto: { color: theme.ink.onAccent, fontWeight: theme.base.weight.semibold },
}));
