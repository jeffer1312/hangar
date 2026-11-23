package expo.modules.porttunnel

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.ProfileStore
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import expo.modules.kotlin.AppContext
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.viewevent.EventDispatcher
import expo.modules.kotlin.views.ExpoView

class PortTunnelModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("PortTunnel")

    Function("isSupported") { WebViewFeature.isFeatureSupported(WebViewFeature.PROXY_OVERRIDE) }

    View(PortTunnelView::class) {
      Events("onNavigation", "onFailure")
      Prop("serverKey") { view: PortTunnelView, value: String -> view.serverKey = value }
      Prop("tunnelUrl") { view: PortTunnelView, value: String -> view.tunnelUrl = value }
      Prop("url") { view: PortTunnelView, value: String -> view.url = value }
      Prop("ports") { view: PortTunnelView, value: List<Int> -> view.ports = value.toSet() }
      OnViewDidUpdateProps { view: PortTunnelView -> view.apply() }
      OnViewDestroys { view: PortTunnelView -> view.destroy() }
    }
  }
}

/**
 * O proxy do WebView vale para todas as WebViews do app e aponta para um SOCKS só: fica posto enquanto
 * houver uma página do túnel na tela e leva ao servidor da última que entrou. Só na thread principal.
 */
// ponytail: duas páginas de servidores diferentes empilhadas usam o túnel da de cima; uma por servidor exigiria SOCKS que soubesse a origem.
internal object Tunnel {
  private val views = mutableListOf<PortTunnelView>()
  private var listener: SocksListener? = null
  private var applied = false
  private val waiting = mutableListOf<() -> Unit>()

  @Volatile var access: String = ""
    private set

  /** Portas das sessões com página aberta: o SOCKS não tem senha e outro app do aparelho chega a ele. */
  @Volatile var allowed: Set<Int> = emptySet()
    private set

  val lastFailure: String? get() = listener?.lastFailure

  fun attach(view: PortTunnelView, ready: () -> Unit) {
    views.remove(view)
    views.add(view)
    update()
    if (applied) return ready()
    waiting.add(ready)
    if (listener != null) return
    val listener = SocksListener({ access }, { allowed })
    this.listener = listener
    val onSet = Runnable {
      Handler(Looper.getMainLooper()).post {
        if (this.listener !== listener) return@post
        applied = true
        waiting.toList().also { waiting.clear() }.forEach { it() }
      }
    }
    // `*` manda tudo direto e `removeImplicitRules` devolve o loopback ao proxy: o `*,<-loopback>` do desktop.
    // WebView antigo recusa `socks5://`; o `socks://` dele é SOCKS4, que o listener também fala.
    for (scheme in listOf("socks5", "socks")) {
      val config = ProxyConfig.Builder()
        .addProxyRule("$scheme://127.0.0.1:${listener.port}")
        .addBypassRule("*")
        .removeImplicitRules()
        .build()
      if (runCatching { ProxyController.getInstance().setProxyOverride(config, { it.run() }, onSet) }.isSuccess) return
    }
    detachAll("proxy_override")
  }

  /** A rota pode mudar (rede local, Tailscale, Connect): vale a URL da página de cima. */
  fun update() {
    views.lastOrNull()?.tunnelUrl?.takeIf { it.isNotEmpty() }?.let { access = it }
    allowed = views.flatMapTo(HashSet()) { it.ports }
  }

  fun detach(view: PortTunnelView) {
    views.remove(view)
    update()
    if (views.isEmpty()) close()
  }

  private fun detachAll(code: String) {
    val failed = views.toList()
    close()
    views.clear()
    failed.forEach { it.fail(code, "") }
  }

  private fun close() {
    // Tirado mesmo se ainda não aplicou, para nunca sobreviver às páginas.
    if (listener != null) runCatching { ProxyController.getInstance().clearProxyOverride({ it.run() }) {} }
    listener?.close()
    listener = null
    applied = false
    waiting.clear()
  }
}

@SuppressLint("SetJavaScriptEnabled")
class PortTunnelView(context: Context, appContext: AppContext) : ExpoView(context, appContext) {
  override val shouldUseAndroidLayout = true

  private val onNavigation by EventDispatcher()
  private val onFailure by EventDispatcher()
  var serverKey = ""
  var tunnelUrl = ""
  var url = ""
  var ports: Set<Int> = emptySet()
  private var webView: WebView? = null
  private var proxied = false
  private var loaded: String? = null
  private var loading = false

  fun apply() {
    if (serverKey.isEmpty() || tunnelUrl.isEmpty()) return
    if (webView == null) create()
    if (isAttachedToWindow) Tunnel.update()
    load()
  }

  private fun create() {
    val webView = WebView(context)
    // O perfil vem antes de qualquer outro uso da WebView: cookies e storage separados por servidor.
    if (WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE)) {
      val profile = "hangar-port-" + serverKey.replace(Regex("[^A-Za-z0-9_-]"), "_")
      ProfileStore.getInstance().getOrCreateProfile(profile)
      WebViewCompat.setProfile(webView, profile)
    }
    webView.settings.javaScriptEnabled = true
    webView.settings.domStorageEnabled = true
    webView.webViewClient = object : WebViewClient() {
      override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) = report(true)
      override fun onPageFinished(view: WebView, url: String?) = report(false)
      override fun doUpdateVisitedHistory(view: WebView, url: String?, isReload: Boolean) = report(null)
      override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
        if (request.isForMainFrame) fail(Tunnel.lastFailure ?: "network", error.description.toString())
      }
    }
    webView.webChromeClient = object : WebChromeClient() {
      override fun onReceivedTitle(view: WebView, title: String?) = report(null)
    }
    addView(webView, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
    this.webView = webView
    if (isAttachedToWindow) attachTunnel()
  }

  internal fun fail(code: String, description: String) {
    // Recarregar depois da falha tem de pedir de novo, mesmo com a mesma URL.
    loaded = null
    onFailure(mapOf("code" to code, "description" to description))
  }

  private fun report(nowLoading: Boolean?) {
    val webView = webView ?: return
    if (nowLoading != null) loading = nowLoading
    onNavigation(mapOf("url" to (webView.url ?: ""), "title" to (webView.title ?: ""), "loading" to loading))
  }

  private fun attachTunnel() {
    Tunnel.attach(this) {
      proxied = true
      load()
    }
  }

  override fun onAttachedToWindow() {
    super.onAttachedToWindow()
    if (webView != null) attachTunnel()
  }

  override fun onDetachedFromWindow() {
    super.onDetachedFromWindow()
    proxied = false
    Tunnel.detach(this)
  }

  private fun load() {
    val webView = webView ?: return
    if (!proxied || url.isEmpty() || url == loaded) return
    loaded = url
    webView.loadUrl(url)
  }

  fun destroy() {
    Tunnel.detach(this)
    webView?.let {
      removeView(it)
      it.destroy()
    }
    webView = null
  }
}
