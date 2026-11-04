import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, Animated, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { formatElapsed, type AgentRun, type ChatEvent, type CodexCommand } from '@hangar/core';
import { WorkingMark } from '../ui/WorkingMark';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import * as m from '../paraglide/messages';
import { workingLabel } from './liveWork';
import { agentIdentities, agentModel, matchAgentSubagent } from '../features/activity/agentModel';
import { useAgentSubagents, type AgentSubagents } from '../features/activity/useAgentSubagents';

// Bloco "trabalhando" do fim da conversa (porte do fim do MessageList.svelte): linha de spinner com
// o verbo do terminal e os segundos, o raciocínio chegando e os subagentes que ainda rodam.

export function CodexCommandsChip({ commands, onPress }: { commands: CodexCommand[]; onPress: () => void }) {
  const { theme } = useUnistyles();
  if (!commands.length) return null;
  const background = commands.filter(command => command.background).length;
  const label = background ? m.codex_terminals_count({ n: background }) : m.codex_commands_count({ n: commands.length });
  return <Pressable onPress={onPress} style={[styles.linha, { minHeight: 44 }]} accessibilityRole="button" accessibilityLabel={label}>
    <Icon name="Terminal" size={16} color={theme.tokens.accent.base} />
    <Text style={[styles.verbo, { color: theme.tokens.text.secondary }]}>{label}</Text>
    <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} />
  </Pressable>;
}

// Relógio só de quem mostra segundos: o tique re-renderiza a linha, não a lista.
function useAgora(): number {
  const [agora, setAgora] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setAgora(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return agora;
}

function Pulso({ children }: { children: ReactNode }) {
  const op = useRef(new Animated.Value(1)).current;
  useEffect(() => {
    const l = Animated.loop(Animated.sequence([
      Animated.timing(op, { toValue: 0.4, duration: 900, useNativeDriver: true }),
      Animated.timing(op, { toValue: 1, duration: 900, useNativeDriver: true }),
    ]));
    l.start();
    return () => l.stop();
  }, [op]);
  return <Animated.View style={{ opacity: op }}>{children}</Animated.View>;
}

/** "Pensando… 12s": texto e tempo do rótulo do terminal; sem tempo nele, conta do começo do turno. */
export function WorkingLine({ label, since }: { label?: string | null; since: number | null }) {
  const { theme } = useUnistyles();
  const agora = useAgora();
  const lido = workingLabel(label);
  const verbo = lido.text ?? m.native_working_line();
  const tempo = lido.elapsed ?? (since !== null ? formatElapsed(Math.max(0, agora - since) / 1000) : '');
  return (
    <View style={styles.linha} accessibilityRole="text" accessibilityLiveRegion="polite" accessibilityLabel={verbo}>
      <WorkingMark size={16} color={theme.tokens.accent.base} />
      <Text style={[styles.verbo, { color: theme.tokens.text.secondary }]} numberOfLines={1}>{verbo}</Text>
      {tempo ? <Text style={[styles.tempo, { color: theme.tokens.text.muted }]}>{tempo}</Text> : null}
    </View>
  );
}

/** Raciocínio chegando (Claude sem terminal). Some quando o bloco cai no transcript, então o tempo
 *  conta da montagem. Só a cauda: o texto inteiro a cada pedaço é trabalho à toa. */
export function PensamentoVivo({ texto }: { texto: string }) {
  const { theme } = useUnistyles();
  const inicio = useRef(Date.now()).current;
  const agora = useAgora();
  const cauda = texto.length > 280 ? '…' + texto.slice(-280) : texto;
  return (
    <View style={styles.pensamento} accessibilityLiveRegion="none">
      <View style={styles.linha}>
        <Pulso><Icon name="Brain" size={15} color={theme.tokens.accent.base} /></Pulso>
        <Text style={[styles.verbo, { color: theme.tokens.text.secondary }]}>{m.pensamento_vivo()}</Text>
        <Text style={[styles.tempo, { color: theme.tokens.text.muted }]}>
          {formatElapsed(Math.max(0, agora - inicio) / 1000)}
        </Text>
      </View>
      <Text style={[styles.cauda, { color: theme.tokens.text.muted }]} numberOfLines={4}>{cauda}</Text>
    </View>
  );
}

/** Subagentes ainda rodando, grudados no fim até acabarem de verdade: um Agent em background
 *  devolve resultado na hora e o cartão dele subia com a conversa, com a sessão parecendo parada. */
export const AgentesRodando = memo(function AgentesRodando({ agentes, inicio, onAbrir, serverId, sessionName, events = EMPTY_EVENTS, eventsRevision, subagentSnapshot }: {
  agentes: AgentRun[];
  inicio: (id: string) => number | null;
  onAbrir?: () => void;
  serverId?: string;
  sessionName?: string;
  events?: ChatEvent[];
  eventsRevision?: number;
  subagentSnapshot?: AgentSubagents;
}) {
  const { theme } = useUnistyles();
  const agora = useAgora();
  const conversation = events.find(event => event.kind !== 'notice' && !event.id.startsWith('queued-'))?.id;
  const localSubagents = useAgentSubagents(serverId, sessionName, subagentSnapshot === undefined && agentes.length > 0, conversation,
    agentes.some(agent => !!agent.agentPath));
  const { subs, error, retry } = subagentSnapshot ?? localSubagents;
  const ids = useMemo(() => agentIdentities(events), [events, eventsRevision]);
  return (
    // Sem região viva: o rótulo de cada agente leva o relógio, e o leitor de tela anunciaria a cada segundo.
    <View style={styles.agentes}>
      {agentes.map((a) => {
        const model = agentModel(a, matchAgentSubagent(subs, a, ids));
        const t0 = inicio(a.id);
        const tempo = a.waiting ? m.tool_agente_esperando() : t0 !== null ? m.tool_executando_ha({ tempo: formatElapsed(Math.max(0, agora - t0) / 1000) }) : m.estado_em_execucao();
        return <AgentCard key={a.id} description={a.description} tag={a.subagentType} model={model} tempo={tempo} waiting={!!a.waiting} onPress={onAbrir} />;
      })}
      {error ? <View>
        <Text style={[styles.model, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{error}</Text>
        <Pressable onPress={retry} style={styles.retry} accessibilityRole="button">
          <Text style={[styles.model, { color: theme.tokens.accent.base }]}>{m.sync_retry()}</Text>
        </Pressable>
      </View> : null}
    </View>
  );
});

/** Cartão de um subagente rodando, sem estado: a conversa e a animação da novidade desenham com ele. */
export function AgentCard({ description, tag, model, tempo, waiting = false, onPress }: {
  description?: string | null; tag?: string | null; model?: string | null; tempo: string; waiting?: boolean; onPress?: () => void;
}) {
  const { theme } = useUnistyles();
  return (
    <Pressable
      onPress={onPress}
      disabled={!onPress}
      style={({ pressed }) => [styles.agente, { borderColor: theme.tokens.border.subtle }, pressed && { backgroundColor: theme.tokens.bg.hover }]}
      accessibilityRole={onPress ? 'button' : undefined}
      accessibilityLabel={[description, tag, model, tempo].filter(Boolean).join(', ')}
      accessibilityHint={onPress ? m.tool_abrir_agente() : undefined}
    >
      {waiting ? <Icon name="Hourglass" size={16} color={theme.tokens.text.muted} /> : <ActivityIndicator size="small" color={theme.tokens.accent.base} />}
      <View style={styles.agenteCorpo}>
        <View style={styles.agenteCab}>
          <Text style={[styles.agenteDesc, { color: theme.tokens.text.primary }]} numberOfLines={1}>
            {description || m.atividade_subagente()}
          </Text>
          {tag ? (
            <Text style={[styles.tag, { color: theme.tokens.text.secondary, backgroundColor: superficie(theme, 0.8) }]} numberOfLines={1}>
              {tag}
            </Text>
          ) : null}
        </View>
        {model ? <Text style={[styles.model, { color: theme.tokens.text.muted }]} numberOfLines={1}>{model}</Text> : null}
        <Text style={[styles.tempo, { color: waiting ? theme.tokens.text.muted : theme.tokens.accent.base }]} numberOfLines={1}>{tempo}</Text>
      </View>
      {onPress ? <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} /> : null}
    </Pressable>
  );
}

const EMPTY_EVENTS: ChatEvent[] = [];

const styles = StyleSheet.create((theme) => ({
  linha: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 32 },
  verbo: { flexShrink: 1, fontSize: theme.base.text.xs },
  tempo: { flexShrink: 0, fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] },
  pensamento: { gap: theme.base.space[0.5] },
  cauda: { fontSize: theme.base.text.xs, fontStyle: 'italic', paddingLeft: 23 },
  agentes: { gap: theme.base.space[1] },
  agente: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    minHeight: 44,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderWidth: 1,
    borderRadius: theme.base.radius.sm,
  },
  agenteCorpo: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  agenteCab: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minWidth: 0 },
  agenteDesc: { flexShrink: 1, fontSize: theme.base.text.sm },
  model: { ...theme.type.caption, fontFamily: theme.base.fontMono },
  retry: { minHeight: 44, minWidth: 44, justifyContent: 'center', alignSelf: 'flex-start' },
  tag: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxxs, paddingHorizontal: theme.base.space[1.5], paddingVertical: 1, borderRadius: theme.base.radius.full, overflow: 'hidden' },
}));
