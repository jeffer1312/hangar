import { useCallback, useMemo, useRef, useState } from 'react';
import { Pressable, ScrollView, Share, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import { WebView, type WebViewMessageEvent } from 'react-native-webview';
import type { Edge } from 'react-native-safe-area-context';
import { pageFileName, parseHostMessage, type HtmlPageRef } from '@hangar/core';
import { canShareFile, shareTextFile } from '../../../../src/features/attachments/mediaCache';
import { toast } from '../../../../src/ui/Toast';
import { Screen } from '../../../../src/ui/Screen';
import { useAparencia } from '../../../../src/stores/aparencia';
import { hasImageBackground } from '../../../../src/theme/aplicarMaterial';
import { HeaderBack, useGlassHeader } from '../../../../src/ui/glassHeader';
import { Icon } from '../../../../src/ui/Icon';
import { onlyInitial, usePageDocument, usePageWebView } from '../../../../src/chat/tools/pageDocument';
import { one, pageFromParams } from '../../../../src/chat/tools/pageRoute';
import * as m from '../../../../src/paraglide/messages';

// Libera a pinça mesmo em página que travou o zoom no viewport: ampliar é o motivo da tela cheia.
const ALLOW_PINCH = "(()=>{const f=()=>{let v=document.querySelector('meta[name=viewport]');"
  + "if(!v){v=document.createElement('meta');v.name='viewport';document.head.appendChild(v)}"
  + "v.content='width=device-width,initial-scale=1,minimum-scale=0.5,maximum-scale=5,user-scalable=yes'};"
  + "document.readyState==='loading'?document.addEventListener('DOMContentLoaded',f):f()})();true";

// A página começa abaixo do cabeçalho: a WebView não sabe recuar por baixo do vidro.
const EDGES: Edge[] = ['bottom', 'left', 'right'];

// Página publicada pelo agente em tela própria, como o T3 no celular: voltar no cabeçalho e no gesto, zoom por pinça.
export default function PageScreen() {
  const params = useLocalSearchParams<{ server: string; name: string; id: string; title: string; own?: string }>();
  const page = pageFromParams(params);
  const serverId = one(params.server);
  const sessionName = one(params.name);
  return page
    ? <PageView page={page} serverId={serverId} sessionName={sessionName} />
    : <Shell title="" serverId={serverId} sessionName={sessionName}><Text style={styles.note}>{m.page_error({ title: '' })}</Text></Shell>;
}

function Shell({ title, serverId, sessionName, actions, children }:
  { title: string; serverId: string; sessionName: string; actions?: React.ReactNode; children: React.ReactNode }) {
  const router = useRouter();
  const header = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const { theme } = useUnistyles();
  // Sobre a foto de fundo a página transparente perde a letra na imagem: fundo liso, como no cartão.
  const backed = useAparencia(hasImageBackground);
  const back = useCallback(() => router.replace(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(sessionName)}` as never), [router, serverId, sessionName]);
  const canGoBack = router.canGoBack();
  const headerOptions = useMemo(() => ({
    ...header,
    title,
    headerLeft: canGoBack ? undefined : () => <HeaderBack onPress={back} />,
    headerRight: actions ? () => actions : undefined,
  }), [header, title, canGoBack, back, actions]);
  return (
    <Screen edges={EDGES}>
      <Stack.Screen options={headerOptions} />
      <View style={[styles.flex, { paddingTop: headerHeight }, backed && { backgroundColor: theme.tokens.bg.base }]}>{children}</View>
    </Screen>
  );
}

function PageView({ page, serverId, sessionName }: { page: HtmlPageRef; serverId: string; sessionName: string }) {
  const { theme } = useUnistyles();
  const { state, doc, load, params, loadSource } = usePageDocument(page, sessionName, serverId);
  const { web, applyTheme, touched, openLink } = usePageWebView(params, page.ownTheme);
  // Fonte: o código da página, para ler, copiar e compartilhar (no iOS como arquivo .html, que a folha salva em Arquivos).
  const [showingSource, setShowingSource] = useState(false);
  const [source, setSource] = useState<string | null>(null);
  const [sourceFailed, setSourceFailed] = useState(false);
  // Um pedido por vez: alternar com o pedido no ar não dispara outro, e uma resposta velha não sobrescreve a nova.
  const fetching = useRef(false);
  const fetchSource = useCallback(async () => {
    if (fetching.current) return;
    fetching.current = true;
    setSourceFailed(false);
    try { setSource(await loadSource()); } catch { setSourceFailed(true); } finally { fetching.current = false; }
  }, [loadSource]);
  const toggleSource = useCallback(() => {
    setShowingSource((v) => !v);
    if (!showingSource && source == null) void fetchSource();
  }, [showingSource, source, fetchSource]);
  const copy = useCallback(async () => {
    if (source == null) return;
    try {
      await Clipboard.setStringAsync(source);
      toast.ok(m.toast_copiado());
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : String(e));
    }
  }, [source]);
  // Cancelar resolve com `dismissedAction`; rejeição aqui é falha de verdade.
  const share = useCallback(async () => {
    if (source == null) return;
    try {
      if (canShareFile) await shareTextFile(source, pageFileName(page.title));
      else await Share.share({ title: pageFileName(page.title), message: source });
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : String(e));
    }
  }, [source, page.title]);
  const c = theme.tokens.text.secondary;
  const accent = theme.tokens.accent.base;
  const sourceActionLabel = showingSource ? m.page_view_page() : m.page_view_source();
  const copyLabel = m.page_copy_source();
  const shareLabel = m.page_share_file();
  const actions = useMemo(() => state === 'ready' ? (
    <View style={styles.actions}>
      <Pressable onPress={toggleSource} hitSlop={8} style={styles.action} accessibilityRole="button" accessibilityState={{ selected: showingSource }}>
        <Text style={[styles.actionText, { color: accent }]}>{sourceActionLabel}</Text>
      </Pressable>
      {source != null && (
        <>
          <Pressable onPress={copy} hitSlop={8} style={styles.icon} accessibilityRole="button" accessibilityLabel={copyLabel}>
            <Icon name="Copy" size={18} color={c} />
          </Pressable>
          <Pressable onPress={share} hitSlop={8} style={styles.icon} accessibilityRole="button" accessibilityLabel={shareLabel}>
            <Icon name="Share" size={18} color={c} />
          </Pressable>
        </>
      )}
    </View>
  ) : undefined, [state, toggleSource, showingSource, accent, sourceActionLabel, source, copy, copyLabel, c, share, shareLabel]);

  const onMessage = useCallback((e: WebViewMessageEvent) => {
    const msg = parseHostMessage(e.nativeEvent.data);
    if (msg?.kind === 'link') openLink(msg.url);
  }, [openLink]);

  let body: React.ReactNode;
  if (state === 'loading') {
    body = <View style={styles.center} accessibilityRole="progressbar"><Text style={styles.note}>{m.page_loading({ title: page.title })}</Text></View>;
  } else if (state === 'expired') {
    body = <View style={styles.center}><Text style={styles.note} selectable>{m.page_expired()}</Text></View>;
  } else if (state === 'error') {
    body = (
      <View style={styles.center} accessibilityRole="alert">
        <Text style={styles.note} selectable>{m.page_error({ title: page.title })}</Text>
        <Pressable onPress={() => void load()} style={styles.retry} accessibilityRole="button" hitSlop={8}>
          <Text style={styles.retryText}>{m.page_retry()}</Text>
        </Pressable>
      </View>
    );
  } else {
    const code = showingSource && (
      sourceFailed ? (
        <View style={styles.center} accessibilityRole="alert">
          <Text style={styles.note} selectable>{m.page_source_error()}</Text>
          <Pressable onPress={() => void fetchSource()} style={styles.retry} accessibilityRole="button" hitSlop={8}>
            <Text style={styles.retryText}>{m.page_retry()}</Text>
          </Pressable>
        </View>
      ) : source == null ? (
        <View style={styles.center} accessibilityRole="progressbar"><Text style={styles.note}>{m.page_loading({ title: page.title })}</Text></View>
      ) : (
        <ScrollView style={styles.flex} contentContainerStyle={styles.codeBox}>
          <Text style={[styles.code, { color: theme.tokens.text.primary }]} selectable>{source}</Text>
        </ScrollView>
      )
    );
    body = (
      // A página fica montada atrás da Fonte: voltar a ela não recarrega.
      <View style={styles.flex}>
      {code}
      <View style={[styles.flex, showingSource && styles.hidden]} onTouchStart={touched}>
        <WebView
          ref={web}
          source={{ html: doc }}
          style={styles.web}
          // Mesmas travas do cartão: só o documento inicial carrega, sem janela nova.
          originWhitelist={['*']}
          onShouldStartLoadWithRequest={onlyInitial}
          setSupportMultipleWindows={false}
          onMessage={onMessage}
          onLoadEnd={applyTheme}
          injectedJavaScript={ALLOW_PINCH}
          scalesPageToFit
          setBuiltInZoomControls
          setDisplayZoomControls={false}
          bounces={false}
          automaticallyAdjustContentInsets={false}
        />
      </View>
      </View>
    );
  }
  return <Shell title={page.title} serverId={serverId} sessionName={sessionName} actions={actions}>{body}</Shell>;
}

const styles = StyleSheet.create((theme) => ({
  flex: { flex: 1 },
  web: { flex: 1, backgroundColor: 'transparent' },
  center: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: theme.base.space[2], padding: theme.base.space[6] },
  note: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
  retry: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  retryText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base },
  actions: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  action: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  actionText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  icon: { width: 40, height: 44, alignItems: 'center', justifyContent: 'center' },
  hidden: { display: 'none' },
  codeBox: { padding: theme.base.space[3] },
  code: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, lineHeight: theme.base.text.xs * 1.5 },
}));
