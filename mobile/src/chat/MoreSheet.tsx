import { useRef } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Sheet } from '../ui/Sheet';
import { Icon, type IconName } from '../ui/Icon';
import * as m from '../paraglide/messages';
import { superficie } from '../theme/superficie';
import { isEmptyBand } from '@hangar/core';
import { chatStore } from '../stores/chat';
import { useMods } from '../stores/mods';
import { useServers } from '../stores/servers';

interface Props {
  open: boolean;
  onClose: () => void;
  serverId: string;
  name: string;
  /** Recicla o processo da sessão Claude sem terminal (relê MCP/hooks/settings). Ausente = não se aplica. */
  recarregar?: { bloqueado: boolean; onPress: () => void };
  /** Terminal ⇄ sem terminal na mesma conversa (Claude e Codex); o rótulo é o destino, tirado de `semTerminal`. */
  trocarModo?: { bloqueado: boolean; trocando?: boolean; onPress: () => void };
  /** Linha do orquestrador: sem pane nem entrada, então Terminal, resposta e anexos não têm o que abrir;
   *  o grupo é montado pelo orq, então parear à mão também sai. */
  orq?: boolean;
  /** Os itens que dependem da sessão só aparecem quando têm o que abrir: Terminal precisa de pane,
   *  Limites é da conta Codex e "Sua resposta" reabre uma pergunta que ficou pendente. */
  provider?: string | null;
  semTerminal?: boolean;
  temPergunta?: boolean;
}

type Item = {
  icon: IconName;
  label: string;
  sub?: string;
  route: string;
  onPress?: () => void;
  disabled?: boolean;
};

export function MoreSheet({ open, onClose, serverId, name, recarregar, trocarModo, orq, provider, semTerminal, temPergunta }: Props) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const afterDismiss = useRef<(() => void) | null>(null);
  const canShare = useServers(s => s.servers.some(server => server.id === serverId && !server.invite && !server.removed));
  const chat = chatStore(serverId, name);
  const surface = chat.use(s => s.pluginUi);
  const dead = chat.use(s => s.stateEvent?.state === 'dead');
  const visible = useMods(s => s.visible);
  const panes = surface?.panes.length ?? 0;
  const hasMods = !dead && !!surface && (!isEmptyBand(surface.above) || panes > 0);
  const modsLabel = visible ? m.mods_celular_ocultar() : panes === 1 ? m.mods_celular_mostrar_um()
    : panes > 1 ? m.mods_celular_mostrar_paineis({ n: String(panes) }) : m.mods_celular_mostrar();

  const items: Item[] = [
    { icon: 'Search', label: m.switcher_buscar_conversas(), route: 'search', onPress: () => router.push('/search' as never) },
    ...(hasMods ? [{ icon: 'PanelsTopLeft' as IconName, label: modsLabel, sub: m.mods_celular_sub(), route: 'mods',
      onPress: () => useMods.getState().setVisible(!visible) }] : []),
    ...(recarregar ? [{
      icon: 'RefreshCw' as IconName,
      label: m.recarregar_sessao(),
      sub: recarregar.bloqueado ? m.modo_so_ociosa() : m.recarregar_sessao_detalhe(),
      route: 'recarregar',
      onPress: recarregar.onPress,
      disabled: recarregar.bloqueado,
    }] : []),
    ...(orq || !temPergunta ? [] : [{ icon: 'CircleHelp' as IconName, label: m.askq_sua_resposta(), route: 'ask' }]),
    ...(canShare ? [{ icon: 'Users' as IconName, label: m.orqcfg_aba_papeis(), route: 'orchestration-team' }] : []),
    orq
      ? { icon: 'PanelsTopLeft', label: m.orq_tab_title(), route: 'orchestration' }
      : { icon: 'Activity', label: m.ctx_atividade(), sub: m.more_tarefas_agentes(), route: 'activity' },
    ...(orq ? [] : [{ icon: 'Users' as IconName, label: m.par_titulo(), sub: m.ctx_grupo(), route: 'pair' }]),
    ...(!orq && canShare ? [{ icon: 'Share2' as IconName, label: m.compartilhar_menu(), route: 'share' }] : []),
    { icon: 'Folder', label: m.arq_aba(), sub: m.ctx_repositorio(), route: 'files' },
    ...(canShare ? [{ icon: 'GitBranch' as IconName, label: m.sessao_git(), sub: m.chat_acao_git_detalhe(), route: 'git' }] : []),
    ...(orq || semTerminal ? [] : [{ icon: 'Terminal' as IconName, label: m.term_titulo(), sub: m.ctx_terminal(), route: 'terminal' }]),
    // A rota é só do dono: convidado não troca o modo (como no nativo).
    ...(!orq && canShare && trocarModo ? [{
      icon: (semTerminal ? 'Terminal' : 'Cloud') as IconName,
      label: semTerminal ? m.modo_abrir_no_terminal() : m.modo_continuar_sem_terminal(),
      sub: trocarModo.trocando ? m.modo_trocando() : trocarModo.bloqueado ? m.modo_so_ociosa()
        : semTerminal ? m.modo_abrir_no_terminal_detalhe() : m.modo_continuar_sem_terminal_detalhe(),
      route: 'modo',
      onPress: trocarModo.onPress,
      disabled: trocarModo.bloqueado,
    }] : []),
    ...(orq ? [] : [{ icon: 'Paperclip' as IconName, label: m.ctx_anexos(), sub: m.more_fotos_videos_arquivos(), route: 'attachments' }]),
    ...(provider === 'codex' ? [{ icon: 'Gauge' as IconName, label: m.codex_limites_titulo(), sub: m.ctx_limites(), route: 'codex-limits' }] : []),
    ...(!orq && canShare && provider === 'claude' ? [{ icon: 'RefreshCw' as IconName, label: m.bastao_menu(), sub: m.conta_mesma_conversa_sub(), route: 'account' }] : []),
    { icon: 'Flag', label: m.bastao_dossie_titulo(), sub: m.bastao_dossie_sub(), route: 'bastao' },
  ];

  const go = (it: Item) => {
    // O iOS precisa concluir o fechamento antes de apresentar outra tela.
    afterDismiss.current = () => {
      if (it.onPress) it.onPress();
      else router.push(`/s/${serverId}/${name}/${it.route}` as never);
    };
    onClose();
  };

  return (
    <Sheet open={open} sizes={['auto']} onDismiss={() => {
      const action = afterDismiss.current;
      afterDismiss.current = null;
      onClose();
      action?.();
    }}>
      <View style={styles.inner}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.navbar_mais_acoes()}</Text>
        {items.map((it) => (
          <Pressable
            key={it.route}
            onPress={() => go(it)}
            disabled={it.disabled}
            style={[styles.item, it.disabled ? { opacity: 0.5 } : null]}
            accessibilityRole="button"
            accessibilityLabel={it.label}
            accessibilityState={{ disabled: !!it.disabled }}
          >
            <View style={[styles.ico, { backgroundColor: superficie(theme, 0.8) }]}>
              <Icon name={it.icon} size={20} color={theme.tokens.text.secondary} />
            </View>
            <View style={styles.txt}>
              <Text style={[styles.label, { color: theme.tokens.text.primary }]}>{it.label}</Text>
              {it.sub ? <Text style={[styles.sub, { color: theme.tokens.text.muted }]} numberOfLines={1}>{it.sub}</Text> : null}
            </View>
            <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} />
          </Pressable>
        ))}
      </View>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: {
    gap: theme.base.space[1],
    padding: theme.base.space[3],
  },
  title: {
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
    marginBottom: theme.base.space[2],
  },
  item: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 56,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  ico: {
    width: 36,
    height: 36,
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  txt: {
    flex: 1,
    gap: 1,
  },
  label: {
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  sub: {
    fontSize: theme.base.text.xs,
  },
}));
