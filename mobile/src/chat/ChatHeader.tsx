import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useLocalSearchParams } from 'expo-router';
import { cwdParts, rotuloEstado, type State, type ThemeTokens } from '@hangar/core';
import * as m from '../paraglide/messages';
import { TourAnchor } from '../features/tour/TourAnchor';
import { Icon } from '../ui/Icon';
import { EdgeButton } from '../ui/EdgeButton';
import { SessionAvatar } from '../features/sessions/SessionAvatar';
import { WorkingMark } from '../ui/WorkingMark';
import { LiveVoiceChatButton } from '../features/voice/LiveVoiceChatButton';
import { useSessions } from '../stores/sessions';
import { useServers } from '../stores/servers';
import { SPACE } from '../theme/scale';

const PILL: Record<State, keyof ThemeTokens['pill']> = {
  working: 'working',
  idle: 'idle',
  awaiting_input: 'input',
  dead: 'dead',
};

// O loop ocupa outra linha para não espremer o nome da sessão.
export function ChatHeader({
  name,
  state,
  onBack,
  onMore,
  onTitlePress,
  onHome,
  chipLoop,
  header = 'phone',
  onTogglePanel,
  panelOpen,
}: {
  name: string;
  state: State | null;
  onBack: () => void;
  // Só quando o voltar não leva direto ao Início (pilha com mais de uma tela).
  onHome?: () => void;
  onMore: () => void;
  onTitlePress: () => void;
  chipLoop?: React.ReactNode;
  header?: 'phone' | 'tablet';
  onTogglePanel?: () => void;
  panelOpen?: boolean;
}) {
  const { theme } = useUnistyles();
  // Destino lido da rota e da lista: quem monta o cabeçalho não precisa repassar máquina e pasta.
  const params = useLocalSearchParams<{ server?: string | string[] }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : params.server;
  // Campos soltos: a linha inteira muda de identidade a cada publicação da lista.
  const rowServerLabel = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.serverLabel);
  const cwd = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.cwd);
  const provider = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.provider);
  const engine = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.engine);
  const limited = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.limited);
  const serverLabel = useServers((s) => s.servers.find((x) => x.id === serverId)?.label) ?? rowServerLabel ?? '';
  const pasta = cwd ? cwdParts(cwd).base : '';
  const destino = pasta && serverLabel ? `${pasta} @ ${serverLabel}` : pasta || serverLabel;
  const corEstado = (state && theme.tokens.pill?.[PILL[state]]?.fg) || theme.tokens.text.muted;
  // Trabalhando, a marca girando já diz; pronta é o normal. Só o que pede atenção vira palavra, na linha de baixo.
  const aviso = state === 'awaiting_input' || state === 'dead' ? rotuloEstado(state) : '';
  return (
    <View style={styles.wrap}>
      <View style={styles.bar}>
        {header === 'phone' ? <EdgeButton onPress={onBack} hitSlop={8} style={styles.back} accessibilityLabel={m.chat_voltar_sessoes()}>
          <Icon name="ChevronLeft" size={22} color={theme.tokens.text.primary} />
        </EdgeButton> : null}
        {header === 'phone' && onHome ? (
          <EdgeButton onPress={onHome} hitSlop={8} style={styles.back} accessibilityLabel={m.chat_ir_inicio()}>
            <Icon name="House" size={19} color={theme.tokens.text.primary} />
          </EdgeButton>
        ) : null}
        <Pressable
          onPress={onTitlePress}
          style={({ pressed }) => [styles.titulo, pressed && styles.tocado]}
          accessibilityRole="button"
          // Nome, estado e destino completos para o leitor de tela; a ação vai na dica.
          accessibilityLabel={[name, state ? rotuloEstado(state) : '', limited ? m.sessao_limite() : '', serverLabel, cwd].filter(Boolean).join(', ')}
          accessibilityHint={m.sessao_trocar_de()}
        >
          {/* Trabalhando, a marca do Hangar girando diz mais que o logo; parada, o agente aparece. */}
          {state === 'working'
            ? <View style={styles.marca} testID="chat-working-mark"><WorkingMark size={24} color={corEstado} /></View>
            : <View style={state === 'dead' && styles.apagado}><SessionAvatar session={{ provider, engine, limited, state }} size={30} /></View>}
          <View style={styles.textos}>
            <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>
              {name}
            </Text>
            {destino || aviso ? (
              <View style={styles.subtitle}>
                {aviso || pasta ? <Text style={[styles.destino, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                  {aviso ? (
                    <Text testID="chat-state-word" style={[styles.aviso, state === 'awaiting_input' && { color: theme.tokens.status.warning }]}>
                      {aviso}
                    </Text>
                  ) : null}
                  {aviso && destino ? ' · ' : ''}
                  {pasta}{pasta && serverLabel ? ' @ ' : ''}
                </Text> : null}
                {serverLabel ? <>
                  <View style={styles.serverIcon} accessibilityElementsHidden importantForAccessibility="no">
                    <Icon name="Monitor" size={SPACE[3]} color={theme.tokens.text.muted} />
                  </View>
                  <Text style={[styles.destino, { color: theme.tokens.text.muted }]} numberOfLines={1}>{serverLabel}</Text>
                </> : null}
              </View>
            ) : null}
          </View>
        </Pressable>
        {header === 'phone' && serverId ? <LiveVoiceChatButton serverId={serverId} name={name} /> : null}
        {header === 'tablet' ? (
          <Pressable onPress={onTogglePanel} hitSlop={8} style={styles.more}
                     accessibilityRole="button" accessibilityLabel={m.ctx_painel_titulo()} accessibilityState={{ expanded: panelOpen }}>
            <Icon name="PanelRight" size={18} color={theme.tokens.text.secondary} />
          </Pressable>
        ) : null}
        <TourAnchor id="chat-more">
          <Pressable
            onPress={onMore}
            hitSlop={8}
            style={styles.more}
            accessibilityRole="button"
            accessibilityLabel={m.navbar_mais_acoes()}
          >
            <Icon name="Ellipsis" size={18} color={theme.tokens.text.secondary} />
          </Pressable>
        </TourAnchor>
      </View>
      {chipLoop ? <View style={[styles.chips, header === 'tablet' && styles.chipsTablet, header === 'phone' && onHome && styles.chipsHome]}>{chipLoop}</View> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    borderBottomWidth: 1,
    borderBottomColor: theme.tokens.border.subtle,
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[1],
    paddingVertical: theme.base.space[1],
    minHeight: 48,
  },
  // Linha discreta do loop, alinhada ao nome (depois do ‹ e da marca).
  chips: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingLeft: theme.base.space[10] * 2 + theme.base.space[2] + theme.base.space[0.5], // coluna do nome: recuo + ‹ + avatar + vãos
    paddingRight: theme.base.space[2],
    paddingBottom: theme.base.space[1],
  },
  chipsTablet: { paddingLeft: theme.base.space[10] + theme.base.space[1] + theme.base.space[0.5] },
  chipsHome: { paddingLeft: theme.base.space[10] * 3 + theme.base.space[2] + theme.base.space[0.5] },
  marca: { width: 30, height: 30, alignItems: 'center', justifyContent: 'center' },
  titulo: {
    flex: 1,
    flexShrink: 1,
    minWidth: 0,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    minHeight: 44,
    paddingHorizontal: theme.base.space[1],
    borderRadius: theme.base.radius.md,
  },
  textos: {
    flex: 1,
    minWidth: 0,
    justifyContent: 'center',
  },
  destino: {
    fontSize: theme.base.text.xxs,
    flexShrink: 1,
  },
  subtitle: { flexDirection: 'row', alignItems: 'center', minWidth: 0 },
  serverIcon: { flexShrink: 0, marginRight: theme.base.space[0.5] },
  tocado: {
    backgroundColor: theme.tokens.bg.hover,
  },
  back: {
    width: 40,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  name: {
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  aviso: {
    fontWeight: theme.base.weight.medium,
  },
  apagado: {
    opacity: 0.5,
  },
  more: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
}));
