import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { basename, fetchWorktreesForServer, getWorktreesForServer, mergedWorktreeBatch, relativeTime,
         WORKTREE_STALE_DAYS, worktreeAgeDays, worktreeIsAgent, worktreeReady, worktreesSizeTotal, worktreeState,
         type WorktreeRepo, type WorktreeStatus } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { useSettingsColors } from '../../src/features/config/colors';
import { WorktreeSheet } from '../../src/features/worktrees/WorktreeSheet';
import { WorktreeBatchSheet, type WorktreeBatch } from '../../src/features/worktrees/WorktreeBatchSheet';
import { putWorktreeStatus } from '../../src/features/worktrees/worktreeStatus';
import { displayTitle, SessionChips, sizeLabel, sizeSumLabel, STATE_TONE, stateLabel, useStateColor } from '../../src/features/worktrees/worktreeUi';
import { Chip } from '../../src/ui/Chip';
import { Icon } from '../../src/ui/Icon';
import { RADIUS } from '../../src/theme/scale';
import { useServers } from '../../src/stores/servers';
import { useSessions } from '../../src/stores/sessions';
import * as m from '../../src/paraglide/messages';

type Filter = 'todas' | 'uso' | 'dirty' | 'prontas' | 'paradas' | 'sem_branch';
const PENDING_REREAD_MS = 5000;
const PENDING_MAX_TRIES = 12;
const DISK_TOP = 5;

const stale = (w: WorktreeStatus) => (worktreeAgeDays(w) ?? 0) > WORKTREE_STALE_DAYS;
const MATCH: Record<Filter, (w: WorktreeStatus) => boolean> = {
  todas: () => true,
  uso: (w) => w.sessions.length > 0,
  dirty: (w) => w.dirty > 0,
  prontas: worktreeReady,
  paradas: stale,
  sem_branch: (w) => !w.branch,
};

export default function Worktrees() {
  const server = useServers((s) => s.active());
  // A leitura segue o id: o store troca o objeto do servidor a cada atualização da lista, e isso
  // não pode reler nem zerar a tela.
  const serverRef = useRef(server);
  serverRef.current = server;
  const serverId = server?.id ?? null;
  const geracao = useRef(0);
  const tentativas = useRef(0);
  const seq = useRef(0);
  const releitura = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [repos, setRepos] = useState<WorktreeRepo[] | null>(null);
  const [erro, setErro] = useState('');
  const [avisoFetch, setAvisoFetch] = useState<string[]>([]);
  const [demorando, setDemorando] = useState(false);
  const [remoto, setRemoto] = useState<{ feitos: number; total: number } | null>(null);
  const [aberta, setAberta] = useState<string | null>(null);
  const [filtro, setFiltro] = useState<Filter>('todas');
  const [agentesAbertos, setAgentesAbertos] = useState<Record<string, boolean>>({});
  // A confirmação congela o que a pessoa viu: o lote apaga essas e só essas.
  const [lote, setLote] = useState<WorktreeBatch | null>(null);
  const c = useSettingsColors();
  const cor = useStateColor();

  // Os chips de sessão leem o estado ao vivo da lista que o app já assina.
  useEffect(() => useSessions.getState().retain(), []);

  const carregar = useCallback(async (): Promise<WorktreeRepo[] | null> => {
    const alvo = serverRef.current;
    if (!alvo) return null;
    const g = geracao.current;
    // Só a resposta do pedido mais novo vale: uma releitura atrasada não desfaz o que um apagar já mostrou.
    const req = ++seq.current;
    const vale = () => g === geracao.current && req === seq.current;
    try {
      const r = await getWorktreesForServer(alvo);
      if (!vale()) return null;
      r.forEach((repo) => repo.worktrees.forEach((w) => putWorktreeStatus(alvo.id, w)));
      setRepos(r);
      setErro('');
      // O backend mede o disco em segundo plano, uma de cada vez: relê até os tamanhos chegarem, com teto.
      if (releitura.current) clearTimeout(releitura.current);
      releitura.current = null;
      const pendente = r.some((repo) => repo.worktrees.some((w) => w.size_pending));
      if (!pendente) tentativas.current = 0;
      else if (tentativas.current < PENDING_MAX_TRIES) {
        tentativas.current++;
        releitura.current = setTimeout(() => { if (g === geracao.current) void carregar(); }, PENDING_REREAD_MS);
      }
      setDemorando(pendente && tentativas.current >= PENDING_MAX_TRIES && !releitura.current);
      return r;
    } catch (e) {
      if (vale()) setErro(e instanceof Error ? e.message : String(e));
      return null;
    }
  }, []);

  const iniciar = useCallback(() => {
    const g = ++geracao.current;
    tentativas.current = 0;
    if (releitura.current) clearTimeout(releitura.current);
    setRepos(null); setErro(''); setAvisoFetch([]); setDemorando(false); setAberta(null); setLote(null); setRemoto(null);
    const alvo = serverRef.current;
    if (!alvo) return;
    void (async () => {
      // Mostra o que já está no disco e só então busca o remoto, que pode demorar.
      const lidos = await carregar();
      if (!lidos || g !== geracao.current) return;
      setRemoto(lidos.length ? { feitos: 0, total: lidos.length } : null);
      const contar = () => { if (g === geracao.current) setRemoto((x) => x && { ...x, feitos: x.feitos + 1 }); };
      const res = await Promise.allSettled(lidos.map((r) => fetchWorktreesForServer(alvo, r.repo).finally(contar)));
      if (g !== geracao.current) return;
      setRemoto(null);
      setAvisoFetch(res.flatMap((x, i) => {
        if (x.status !== 'rejected') return [];
        console.warn('worktrees: fetch falhou', lidos[i].repo, x.reason);
        return [m.worktrees_busca_remoto_falhou({ repo: basename(lidos[i].repo) })];
      }));
      await carregar();
    })();
  }, [carregar]);

  useEffect(() => {
    iniciar();
    return () => {
      // Invalida o que ainda está em voo: nada grava estado nem agenda releitura depois de sair.
      geracao.current++;
      if (releitura.current) clearTimeout(releitura.current);
    };
  }, [serverId, iniciar]);

  const recarregar = () => { tentativas.current = 0; void carregar(); };

  const todas = useMemo(() => repos?.flatMap((r) => r.worktrees) ?? [], [repos]);
  const conta = (f: Filter) => todas.filter(MATCH[f]).length;
  const prontasTamanho = sizeSumLabel(todas.filter(worktreeReady));
  const emUsoNomes = todas.flatMap((w) => w.sessions);
  const contadores: { f: Filter; label: string; hint: string; color: string }[] = [
    { f: 'uso', label: m.worktrees_contador_em_uso(), color: cor('session'),
      hint: emUsoNomes.length ? emUsoNomes.join(' · ') : m.worktrees_contador_em_uso_vazio() },
    { f: 'dirty', label: m.worktrees_contador_nao_commitado(), color: cor('dirty'), hint: m.worktrees_contador_nao_commitado_dica() },
    { f: 'prontas', label: m.worktrees_contador_prontas(), color: cor('merged'), hint: m.worktrees_contador_prontas_dica({ tamanho: prontasTamanho }) },
    { f: 'paradas', label: m.worktrees_contador_paradas(), color: c.muted, hint: m.worktrees_contador_paradas_dica() },
  ];
  const filtros: { f: Filter; label: string }[] = [
    { f: 'todas', label: m.worktrees_filtro_todas() },
    { f: 'uso', label: m.worktrees_filtro_em_uso() },
    { f: 'dirty', label: m.worktrees_filtro_nao_commitado() },
    { f: 'prontas', label: m.worktrees_filtro_prontas() },
    { f: 'paradas', label: m.worktrees_filtro_paradas() },
    { f: 'sem_branch', label: m.worktrees_filtro_sem_branch() },
  ];
  const alternar = (f: Filter) => setFiltro((atual) => (atual === f ? 'todas' : f));

  const visiveis = repos?.map((r) => ({ r, ws: r.worktrees.filter(MATCH[filtro]) })).filter((x) => x.ws.length) ?? [];

  return (
    <Pagina>
      <PageHeader title={m.worktrees_titulo()} subtitle={remoto ? m.worktrees_remoto_progresso(remoto) : undefined} />
      {!server ? <InfoNotice text={m.maquinas_vazio()} /> : null}
      {server && repos === null && !erro ? (
        <View style={styles.lendo} accessible accessibilityRole="progressbar" accessibilityLabel={m.worktrees_lendo()}>
          <ActivityIndicator color={c.muted} />
          <Text style={styles.muted}>{m.worktrees_lendo()}</Text>
        </View>
      ) : null}
      {erro ? (
        <View style={styles.erroBloco}>
          <Text accessibilityRole="alert" style={styles.erro}>{m.worktrees_erro({ motivo: erro })}</Text>
          <Pill icon="RotateCw" label={m.busca_tentar_de_novo()} onPress={iniciar} />
        </View>
      ) : null}
      {avisoFetch.map((t) => <Text key={t} accessibilityRole="alert" style={styles.aviso}>{t}</Text>)}
      {repos && repos.length === 0 ? <InfoNotice text={m.worktrees_vazio()} /> : null}
      {server && repos?.length ? (
        <>
          <View style={styles.grade}>
            {contadores.map((k) => {
              const on = filtro === k.f;
              return (
                <Pressable key={k.f} onPress={() => alternar(k.f)} accessibilityRole="button" accessibilityState={{ selected: on }}
                  accessibilityLabel={`${k.label}: ${conta(k.f)}`}
                  style={({ pressed }) => [styles.contador, { borderColor: on ? k.color : c.border, backgroundColor: c.inset },
                    pressed && { opacity: 0.7 }]}>
                  <View style={styles.contadorTopo}>
                    <View style={[styles.ponto, { backgroundColor: k.color }]} />
                    <Text style={[styles.contadorLabel, { color: c.muted }]} numberOfLines={2}>{k.label}</Text>
                  </View>
                  <Text style={[styles.contadorN, { color: c.text }]}>{conta(k.f)}</Text>
                  <Text style={[styles.contadorHint, { color: c.faint }]} numberOfLines={2}>{k.hint}</Text>
                </Pressable>
              );
            })}
          </View>
          <Disco todas={todas} onPick={setAberta} demorando={demorando} onRetry={recarregar} />
          <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.filtros}>
            {filtros.map((x) => {
              const on = filtro === x.f;
              return (
                <Pressable key={x.f} onPress={() => setFiltro(x.f)} accessibilityRole="button" accessibilityState={{ selected: on }}
                  hitSlop={{ top: 5, bottom: 5 }}
                  style={[styles.filtro, { borderColor: on ? c.borderStrong : c.border }, on && { backgroundColor: c.hover }]}>
                  <Text style={[styles.filtroTxt, { color: on ? c.text : c.muted }]}>{x.label}</Text>
                  <Text style={[styles.filtroN, { color: c.faint }]}>{conta(x.f)}</Text>
                </Pressable>
              );
            })}
          </ScrollView>
          {!visiveis.length ? <InfoNotice text={m.worktrees_filtro_vazio()} /> : null}
          {visiveis.map(({ r, ws }) => {
            const b = mergedWorktreeBatch(r);
            // Mesmo número do contador "Prontas pra apagar"; o lote só aparece sozinho quando não há prontas.
            const prontas = r.worktrees.filter(worktreeReady);
            const [limpar, rotuloLimpar] = prontas.length ? [prontas, m.worktree_limpar_mescladas]
              : [b.deletable, m.worktree_clean_merged_with_changes];
            const principais = ws.filter((w) => !worktreeIsAgent(w));
            const agentes = ws.filter(worktreeIsAgent);
            // Com filtro ligado o grupo abre sozinho: o que casou não pode ficar escondido.
            const agentesVisiveis = filtro !== 'todas' || !!agentesAbertos[r.repo];
            const maisRecente = Math.max(0, ...agentes.map((w) => w.last_commit?.at ?? w.created_at ?? 0));
            const principal = r.worktrees.find((w) => w.main_branch)?.main_branch;
            return (
              <SectionCard
                key={r.repo}
                icon="GitBranch"
                title={basename(r.repo)}
                subtitle={[m.worktree_repo_resumo({ n: r.worktrees.length, tamanho: sizeSumLabel(r.worktrees) }),
                  ...(principal ? [m.worktree_repo_principal({ branch: principal })] : [])].join(' · ')}
              >
                {b.deletable.length ? (
                  <View style={[styles.limpar, { borderTopColor: c.border }]}>
                    <Pill icon="Trash2" onPress={() => setLote({ repo: r.repo, ...b })}
                      label={rotuloLimpar({ n: limpar.length, tamanho: sizeSumLabel(limpar) })} />
                  </View>
                ) : null}
                {principais.map((w) => <Linha key={w.path} w={w} serverId={server.id} onPick={setAberta} />)}
                {agentes.length ? (
                  <>
                    <Pressable onPress={() => setAgentesAbertos((o) => ({ ...o, [r.repo]: !o[r.repo] }))}
                      accessibilityRole="button" accessibilityState={{ expanded: agentesVisiveis }}
                      style={[styles.grupo, { borderTopColor: c.border }]}>
                      <Icon name={agentesVisiveis ? 'ChevronDown' : 'ChevronRight'} size={16} color={c.muted} />
                      <View style={styles.flex}>
                        <Text style={[styles.titulo, { color: c.text }]}>{m.worktree_subagentes()}  <Text style={{ color: c.faint }}>{agentes.length}</Text></Text>
                        {maisRecente ? (
                          <Text style={[styles.meta, { color: c.faint }]}>{m.worktree_subagentes_dica({ quando: relativeTime(maisRecente) })}</Text>
                        ) : null}
                      </View>
                    </Pressable>
                    {agentesVisiveis ? agentes.map((w) => <Linha key={w.path} w={w} serverId={server.id} onPick={setAberta} />) : null}
                  </>
                ) : null}
              </SectionCard>
            );
          })}
          <Text style={[styles.legenda, { color: c.faint }]}>{m.worktrees_legenda_commits()}</Text>
        </>
      ) : null}
      <WorktreeSheet server={server} path={aberta} onClose={() => setAberta(null)} onDeleted={recarregar} />
      <WorktreeBatchSheet server={server} batch={lote} onClose={() => setLote(null)} onDeleted={recarregar} />
    </Pagina>
  );
}

function Linha({ w, serverId, onPick }: { w: WorktreeStatus; serverId: string; onPick: (path: string) => void }) {
  const c = useSettingsColors();
  const k = worktreeState(w);
  const idade = worktreeAgeDays(w);
  const atividade = relativeTime(w.last_commit?.at ?? w.created_at);
  const titulo = displayTitle(w);
  return (
    <View style={[styles.linha, { borderTopColor: c.border }]}>
      <Pressable onPress={() => onPick(w.path)} accessibilityRole="button" accessibilityLabel={`${titulo}, ${stateLabel(k)}`}
        style={({ pressed }) => [styles.linhaCorpo, pressed && { opacity: 0.6 }]}>
        <View style={styles.linhaTopo}>
          <Text style={[styles.titulo, styles.flex, { color: c.text }]} numberOfLines={1}>{titulo}</Text>
          <Text style={[styles.meta, { color: c.muted }]}>{sizeLabel(w)}</Text>
        </View>
        <View style={styles.linhaEstado}>
          <Chip tone={STATE_TONE[k]}>{stateLabel(k)}</Chip>
          {idade != null && stale(w) ? <Text style={[styles.meta, { color: c.faint }]}>{m.worktree_parada_dias({ n: idade })}</Text> : null}
        </View>
        <Text style={[styles.mono, { color: c.muted }]} numberOfLines={1}>
          {w.branch ?? m.worktree_sem_branch_rotulo()} ← {w.base ?? ''}
        </Text>
        {w.last_commit ? <Text style={[styles.meta, { color: c.muted }]} numberOfLines={1}>{w.last_commit.subject}</Text> : null}
        {w.degraded ? <Text style={styles.aviso}>{m.worktree_leitura_incompleta()}</Text> : null}
        <Text style={[styles.meta, { color: c.faint }]} numberOfLines={1}>
          {[`↑${w.ahead} ↓${w.behind ?? 0}`,
            w.residual ? m.worktree_estado_residual() : w.dirty ? m.worktree_n_nao_commitados({ n: w.dirty }) : m.worktree_limpa(),
            ...(atividade ? [atividade] : [])].join(' · ')}
        </Text>
      </Pressable>
      <SessionChips serverId={serverId} names={w.sessions} />
    </View>
  );
}

function Disco({ todas, onPick, demorando, onRetry }: { todas: WorktreeStatus[]; onPick: (path: string) => void; demorando: boolean; onRetry: () => void }) {
  const c = useSettingsColors();
  const cor = useStateColor();
  const medidas = todas.filter((w) => (w.size ?? 0) > 0).sort((a, b) => (b.size ?? 0) - (a.size ?? 0));
  const pendente = todas.some((w) => w.size_pending);
  const existentes = todas.filter((w) => w.exists);
  const medidasN = existentes.filter((w) => !w.size_pending).length;
  const prontas = todas.filter(worktreeReady);
  const maior = medidas[0]?.size ?? 1;
  const topo = medidas.slice(0, DISK_TOP);
  const resto = medidas.slice(DISK_TOP);
  const legenda = [
    { k: 'merged', label: m.worktrees_filtro_prontas() },
    { k: 'dirty', label: m.worktrees_filtro_nao_commitado() },
    { k: 'session', label: m.worktrees_filtro_em_uso() },
    { k: 'active', label: m.worktree_estado_andamento() },
    { k: 'detached', label: m.worktrees_filtro_sem_branch() },
  ] as const;
  return (
    <SectionCard icon="HardDrive" title={m.worktrees_disco_titulo()}
      subtitle={[m.worktrees_disco_resumo({ total: sizeSumLabel(todas), n: medidas.length }),
        ...(worktreesSizeTotal(prontas).bytes ? [m.worktrees_disco_libera({ tamanho: sizeSumLabel(prontas) })] : []),
        ...(pendente ? [demorando ? m.worktrees_disco_demorando()
          : m.worktrees_disco_progresso({ feitas: medidasN, total: existentes.length })] : [])].join(' · ')}>
      <View style={[styles.disco, { borderTopColor: c.border }]}>
        {pendente && demorando ? (
          <View style={styles.erroBloco}><Pill icon="RotateCw" label={m.busca_tentar_de_novo()} onPress={onRetry} /></View>
        ) : null}
        {medidas.length ? (
          <View style={[styles.barra, { backgroundColor: c.inset }]}>
            {medidas.map((w) => <View key={w.path} style={{ flexGrow: w.size ?? 0, backgroundColor: cor(worktreeState(w)) }} />)}
          </View>
        ) : null}
        {topo.map((w) => {
          const k = worktreeState(w);
          return (
            <Pressable key={w.path} onPress={() => onPick(w.path)} accessibilityRole="button"
              accessibilityLabel={`${displayTitle(w)}, ${sizeLabel(w)}, ${stateLabel(k)}`}
              style={({ pressed }) => [styles.discoLinha, pressed && { opacity: 0.6 }]}>
              <View style={styles.linhaTopo}>
                <Text style={[styles.meta, styles.flex, { color: c.text }]} numberOfLines={1}>{displayTitle(w)}</Text>
                <Text style={[styles.meta, { color: c.muted }]}>{sizeLabel(w)}</Text>
              </View>
              <View style={[styles.trilho, { backgroundColor: c.inset }]}>
                <View style={{ width: `${Math.max(1, Math.round(((w.size ?? 0) / maior) * 100))}%` as const, height: '100%', borderRadius: RADIUS.full, backgroundColor: cor(k) }} />
              </View>
            </Pressable>
          );
        })}
        {resto.length ? (
          <View style={[styles.linhaTopo, styles.discoLinha]}>
            <Text style={[styles.meta, styles.flex, { color: c.muted }]}>{m.worktrees_disco_outras({ n: resto.length })}</Text>
            <Text style={[styles.meta, { color: c.muted }]}>{sizeSumLabel(resto)}</Text>
          </View>
        ) : null}
        <View style={styles.legendaLinha}>
          {legenda.map((l) => (
            <View key={l.k} style={styles.legendaItem}>
              <View style={[styles.ponto, { backgroundColor: cor(l.k) }]} />
              <Text style={[styles.meta, { color: c.faint }]}>{l.label}</Text>
            </View>
          ))}
        </View>
      </View>
    </SectionCard>
  );
}

const styles = StyleSheet.create((theme) => ({
  muted: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[1] },
  erro: { fontSize: theme.base.text.sm, color: theme.tokens.status.error, paddingHorizontal: theme.base.space[1] },
  erroBloco: { gap: theme.base.space[2], alignItems: 'flex-start' },
  lendo: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  aviso: { fontSize: theme.base.text.xs, color: theme.tokens.status.warning, paddingHorizontal: theme.base.space[1] },
  flex: { flex: 1, minWidth: 0 },
  grade: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
  contador: { flexBasis: '45%', flexGrow: 1, borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[3], gap: theme.base.space[1] },
  contadorTopo: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5] },
  contadorLabel: { flex: 1, fontSize: theme.base.text.xxs },
  contadorN: { fontSize: theme.base.text.xl, fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] },
  contadorHint: { fontSize: theme.base.text.xxxs },
  ponto: { width: 8, height: 8, borderRadius: theme.base.radius.full },
  filtros: { gap: theme.base.space[2], paddingHorizontal: theme.base.space[0.5] },
  filtro: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], height: 34, paddingHorizontal: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.full },
  filtroTxt: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  filtroN: { fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] },
  limpar: { borderTopWidth: 1, paddingVertical: theme.base.space[2], paddingHorizontal: theme.base.space[4], flexDirection: 'row' },
  linha: { borderTopWidth: 1, paddingVertical: theme.base.space[3], paddingHorizontal: theme.base.space[4], gap: theme.base.space[2] },
  linhaCorpo: { gap: theme.base.space[1] },
  linhaTopo: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  linhaEstado: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], flexWrap: 'wrap' },
  titulo: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  meta: { fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  mono: { fontSize: theme.base.text.xxs, fontFamily: theme.base.fontMono },
  grupo: { borderTopWidth: 1, paddingVertical: theme.base.space[3], paddingHorizontal: theme.base.space[4], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  disco: { borderTopWidth: 1, paddingVertical: theme.base.space[3], paddingHorizontal: theme.base.space[4], gap: theme.base.space[2] },
  barra: { flexDirection: 'row', height: 10, borderRadius: theme.base.radius.full, overflow: 'hidden', gap: 1 },
  discoLinha: { gap: theme.base.space[1], minHeight: 44, justifyContent: 'center' },
  trilho: { height: 6, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  legendaLinha: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2], paddingTop: theme.base.space[0.5] },
  legendaItem: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  legenda: { fontSize: theme.base.text.xxs, paddingHorizontal: theme.base.space[1] },
}));
