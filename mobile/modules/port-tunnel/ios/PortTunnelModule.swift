import ExpoModulesCore
import Network
import WebKit

public final class PortTunnelModule: Module {
  public func definition() -> ModuleDefinition {
    Name("PortTunnel")

    Function("isSupported") { () -> Bool in
      if #available(iOS 17.0, *) { return true }
      return false
    }

    View(PortTunnelView.self) {
      Events("onNavigation", "onFailure")
      Prop("serverKey") { (view: PortTunnelView, value: String) in view.serverKey = value }
      Prop("tunnelUrl") { (view: PortTunnelView, value: String) in view.tunnelUrl = value }
      Prop("url") { (view: PortTunnelView, value: String) in view.url = value }
      Prop("ports") { (view: PortTunnelView, value: [Int]) in view.ports = value }
      OnViewDidUpdateProps { (view: PortTunnelView) in view.apply() }
    }
  }
}

/// WebView cujo loopback é o da máquina da sessão: o `localhost` vai ao SOCKS deste processo, que leva
/// cada conexão pelo túnel do servidor. Só existe no iOS 17+ (o JS confere `isSupported` antes).
final class PortTunnelView: ExpoView, WKNavigationDelegate {
  let onNavigation = EventDispatcher()
  let onFailure = EventDispatcher()
  var serverKey = ""
  var tunnelUrl = ""
  var url = ""
  var ports: [Int] = []

  private var webView: WKWebView?
  private var proxy: ServerProxy?
  private var loaded: URL?
  private var observations: [NSKeyValueObservation] = []

  func apply() {
    guard #available(iOS 17.0, *), !serverKey.isEmpty, !tunnelUrl.isEmpty else { return }
    if proxy == nil {
      let proxy = ServerProxy.of(serverKey)
      self.proxy = proxy
      proxy.ensureListening { [weak self] port in self?.attach(port) }
    }
    proxy?.access.set(tunnelUrl, ports: ports)
    load()
  }

  @available(iOS 17.0, *)
  private func attach(_ port: UInt16?) {
    guard webView == nil, let proxy else { return }
    guard let port else {
      onFailure(["code": "listen", "description": ""])
      return
    }
    let configuration = WKWebViewConfiguration()
    configuration.websiteDataStore = proxy.store(port: port)
    let webView = WKWebView(frame: bounds, configuration: configuration)
    webView.navigationDelegate = self
    webView.allowsBackForwardNavigationGestures = true
    webView.isInspectable = true
    addSubview(webView)
    self.webView = webView
    observations = [
      webView.observe(\.url) { [weak self] _, _ in self?.report() },
      webView.observe(\.title) { [weak self] _, _ in self?.report() },
      webView.observe(\.isLoading) { [weak self] _, _ in self?.report() },
    ]
    load()
  }

  override func layoutSubviews() {
    super.layoutSubviews()
    webView?.frame = bounds
  }

  private func load() {
    guard let webView, let target = URL(string: url).map(Loopback.standIn), target != loaded else { return }
    loaded = target
    webView.load(URLRequest(url: target))
  }

  private func report() {
    guard let webView else { return }
    onNavigation([
      "url": webView.url.map(Loopback.shown)?.absoluteString ?? "",
      "title": webView.title ?? "",
      "loading": webView.isLoading,
    ])
  }

  /// Link ou redirecionamento para o loopback vai ao nome substituto, o único que o proxy leva.
  func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
               decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
    guard let target = action.request.url, action.targetFrame?.isMainFrame ?? true,
          Loopback.standIn(target) != target else { return decisionHandler(.allow) }
    decisionHandler(.cancel)
    var request = action.request
    request.url = Loopback.standIn(target)
    webView.load(request)
  }

  func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
    fail(error)
  }

  func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
    fail(error)
  }

  private func fail(_ error: Error) {
    let error = error as NSError
    if error.domain == NSURLErrorDomain && error.code == NSURLErrorCancelled { return }
    // Recarregar depois da falha tem de pedir de novo, mesmo com a mesma URL.
    loaded = nil
    onFailure(["code": proxy?.listener.lastFailure ?? "network", "description": error.localizedDescription])
  }
}

/// O endereço do túnel muda com a rota (rede local, Tailscale, Connect); cada conexão lê o mais novo.
/// As portas são as da sessão da página: o SOCKS não tem senha e outro app do iPhone chega a ele.
// ponytail: vale a última página aberta deste servidor; duas sessões do mesmo servidor empilhadas ficam com as portas da de cima.
final class TunnelAccess {
  private let lock = NSLock()
  private var url = ""
  private var ports: Set<Int> = []
  func set(_ url: String, ports: [Int]) { lock.lock(); self.url = url; self.ports = Set(ports); lock.unlock() }
  func get() -> String { lock.lock(); defer { lock.unlock() }; return url }
  func allows(_ port: UInt16) -> Bool { lock.lock(); defer { lock.unlock() }; return ports.contains(Int(port)) }
}

/// Um SOCKS e um armazenamento sem disco por servidor, vivos enquanto o app vive: os cookies sobrevivem a
/// fechar a página e nunca chegam às páginas de outro servidor. Só na thread principal.
final class ServerProxy {
  private static var all: [String: ServerProxy] = [:]
  private static var foreground: NSObjectProtocol?

  static func of(_ key: String) -> ServerProxy {
    if let proxy = all[key] { return proxy }
    // O iPhone pode derrubar o socket que escuta enquanto o app está suspenso.
    foreground = foreground ?? NotificationCenter.default.addObserver(
      forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main
    ) { _ in all.values.forEach { $0.ensureListening { _ in } } }
    let proxy = ServerProxy()
    all[key] = proxy
    return proxy
  }

  let access = TunnelAccess()
  lazy var listener = SocksListener(access: access)
  private var port: UInt16?
  private var waiting: [(UInt16?) -> Void] = []
  private var dataStore: WKWebsiteDataStore?

  @available(iOS 17.0, *)
  func store(port: UInt16) -> WKWebsiteDataStore {
    if let dataStore { return dataStore }
    let store = WKWebsiteDataStore.nonPersistent()
    var config = ProxyConfiguration(socksv5Proxy: .hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!))
    // Só o nome substituto vai ao túnel; o resto da página carrega direto.
    config.matchDomains = [Loopback.standInHost]
    config.allowFailover = false
    store.proxyConfigurations = [config]
    dataStore = store
    return store
  }

  /// Sobe o SOCKS, ou o religa na mesma porta (o armazenamento guardou essa porta). `nil` = não subiu.
  func ensureListening(_ ready: @escaping (UInt16?) -> Void) {
    if let port, listener.isListening { return ready(port) }
    waiting.append(ready)
    guard waiting.count == 1 else { return }
    listener.start(port: port) { [weak self] port in
      DispatchQueue.main.async {
        guard let self else { return }
        // ponytail: porta antiga tomada por outro app deixa este servidor sem túnel até reabrir o app.
        if port != nil, self.port == nil { self.port = port }
        let result = port == self.port ? port : nil
        let waiting = self.waiting
        self.waiting = []
        waiting.forEach { $0(result) }
      }
    }
  }
}

/// O iPhone de verdade nunca manda endereço de loopback ao proxy da WebView, então a página roda num nome
/// substituto do mesmo tamanho de `localhost`, e o proxy troca um pelo outro em HTTP puro.
enum Loopback {
  static let standInHost = "hglo.test"

  static func isLoopback(_ host: String) -> Bool {
    let host = host.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
    if host == "localhost" || host.hasSuffix(".localhost") || host == standInHost { return true }
    if let address = IPv4Address(host) { return address.rawValue.first == 127 }
    if let address = IPv6Address(host) { return address.isLoopback }
    return false
  }

  static func standIn(_ url: URL) -> URL {
    guard let host = url.host, host != standInHost, isLoopback(host) else { return url }
    return replacing(url, host: standInHost)
  }

  static func shown(_ url: URL) -> URL {
    url.host?.lowercased() == standInHost ? replacing(url, host: "localhost") : url
  }

  private static func replacing(_ url: URL, host: String) -> URL {
    guard var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url }
    parts.host = host
    return parts.url ?? url
  }
}
