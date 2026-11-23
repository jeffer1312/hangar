import Foundation
import Network

/// SOCKS5 em 127.0.0.1 que leva cada CONNECT ao loopback por um WebSocket do `/api/port-tunnel`. Nunca
/// conecta a nada deste aparelho: túnel que falha é erro, nunca um servidor do próprio iPhone.
// Sem senha, como o SOCKS do desktop: outro app do iPhone alcança a porta, então só as portas da sessão passam.
final class SocksListener {
  private let queue = DispatchQueue(label: "hangar.port-tunnel")
  private let access: TunnelAccess
  private var listener: NWListener?
  private var connections: [ObjectIdentifier: SocksConnection] = [:]
  private var failure: String?

  /// Código da última recusa do túnel, ou nil depois de um túnel que abriu.
  var lastFailure: String? { queue.sync { failure } }
  var isListening: Bool { listener?.state == .ready }

  init(access: TunnelAccess) { self.access = access }

  /// `port` pede uma porta exata: o SOCKS religado fica no endereço que o WebKit já guardou.
  func start(port: UInt16?, done: @escaping (UInt16?) -> Void) {
    listener?.cancel()
    let parameters = NWParameters.tcp
    parameters.allowLocalEndpointReuse = true
    parameters.requiredLocalEndpoint = .hostPort(
      host: "127.0.0.1", port: port.flatMap(NWEndpoint.Port.init(rawValue:)) ?? .any)
    guard let listener = try? NWListener(using: parameters) else { return done(nil) }
    self.listener = listener
    // Responde uma vez só: o handler sai depois da primeira resposta (sem `var` capturada, que o `@Sendable` recusa).
    listener.stateUpdateHandler = { [weak listener] state in
      switch state {
      case .ready:
        listener?.stateUpdateHandler = nil
        done(listener?.port?.rawValue)
      case .failed, .cancelled:
        listener?.stateUpdateHandler = nil
        done(nil)
      default: break
      }
    }
    listener.newConnectionHandler = { [weak self] connection in
      guard let self else { return connection.cancel() }
      let socks = SocksConnection(
        connection: connection, queue: queue, access: access,
        outcome: { [weak self] code in self?.failure = code },
        closed: { [weak self] socks in self?.connections.removeValue(forKey: ObjectIdentifier(socks)) })
      connections[ObjectIdentifier(socks)] = socks
      socks.start()
    }
    listener.start(queue: queue)
  }
}

private func reply(_ code: UInt8) -> Data { Data([5, code, 0, 1, 0, 0, 0, 0, 0, 0]) }

private enum SocksRequest {
  case connect(port: UInt16, consumed: Int)
  case refuse(UInt8)
}

/// nil enquanto o pedido não chegou inteiro.
private func parseConnect(_ data: [UInt8]) -> SocksRequest? {
  guard data.count >= 5 else { return nil }
  let hostLength: Int
  switch data[3] {
  case 1: hostLength = 4
  case 4: hostLength = 16
  case 3: hostLength = 1 + Int(data[4])
  default: return .refuse(8)
  }
  let end = 4 + hostLength + 2
  guard data.count >= end else { return nil }
  guard data[0] == 5, data[1] == 1 else { return .refuse(7) }
  let host = Array(data[4..<(4 + hostLength)])
  let loopback: Bool
  switch data[3] {
  case 1: loopback = host[0] == 127
  case 4: loopback = IPv6Address(Data(host))?.isLoopback ?? false
  default: loopback = Loopback.isLoopback(String(decoding: host.dropFirst(), as: UTF8.self))
  }
  guard loopback else { return .refuse(2) }
  return .connect(port: UInt16(data[end - 2]) << 8 | UInt16(data[end - 1]), consumed: end)
}

/// Troca uma sequência por outra do mesmo tamanho dentro de cada pedaço: o tamanho da mensagem não muda.
/// Um nome partido entre dois pedaços escapa; o cabeçalho `Host` chega sempre inteiro.
private func swapped(_ data: Data, _ from: [UInt8], _ to: [UInt8]) -> Data {
  var bytes = [UInt8](data)
  var i = 0
  while i + from.count <= bytes.count {
    if bytes[i..<(i + from.count)].elementsEqual(from) {
      bytes.replaceSubrange(i..<(i + from.count), with: to)
      i += from.count
    } else {
      i += 1
    }
  }
  return Data(bytes)
}

private let standIn = Array(Loopback.standInHost.utf8)
private let localhost = Array("localhost".utf8)

/// Todos os métodos rodam na fila do listener.
private final class SocksConnection {
  private let connection: NWConnection
  private let queue: DispatchQueue
  private let access: TunnelAccess
  private let outcome: (String?) -> Void
  private let closed: (SocksConnection) -> Void
  private var buffer: [UInt8] = []
  private var greeted = false
  private var tunnel: URLSessionWebSocketTask?
  private var done = false
  /// HTTP puro troca o nome substituto por `localhost`; TLS (primeiro byte 0x16) passa intacto.
  private var plain: Bool?

  init(connection: NWConnection, queue: DispatchQueue, access: TunnelAccess,
       outcome: @escaping (String?) -> Void, closed: @escaping (SocksConnection) -> Void) {
    self.connection = connection
    self.queue = queue
    self.access = access
    self.outcome = outcome
    self.closed = closed
  }

  func start() {
    connection.stateUpdateHandler = { [weak self] state in
      switch state {
      case .failed, .cancelled: self?.close()
      default: break
      }
    }
    connection.start(queue: queue)
    readHandshake()
  }

  func close() {
    guard !done else { return }
    done = true
    tunnel?.cancel(with: .normalClosure, reason: nil)
    connection.cancel()
    closed(self)
  }

  /// Manda o resto e fecha depois de escrito.
  private func finish(_ data: Data?) {
    connection.send(content: data, contentContext: .finalMessage, isComplete: true,
                    completion: .contentProcessed { [weak self] _ in self?.close() })
  }

  private func readHandshake() {
    connection.receive(minimumIncompleteLength: 1, maximumLength: 4096) { [weak self] data, _, isComplete, error in
      guard let self, !done else { return }
      if let data { buffer.append(contentsOf: data) }
      if !greeted {
        guard buffer.count >= 2, buffer.count >= 2 + Int(buffer[1]) else {
          if isComplete || error != nil { close() } else { readHandshake() }
          return
        }
        guard buffer[0] == 5, buffer[2..<(2 + Int(buffer[1]))].contains(0) else { return finish(Data([5, 0xff])) }
        connection.send(content: Data([5, 0]), completion: .idempotent)
        buffer.removeFirst(2 + Int(buffer[1]))
        greeted = true
      }
      switch parseConnect(buffer) {
      case nil:
        if isComplete || error != nil { close() } else { readHandshake() }
      case .refuse(let code)?:
        finish(reply(code))
      case .connect(let port, let consumed)?:
        buffer.removeFirst(consumed)
        open(port)
      }
    }
  }

  private func open(_ port: UInt16) {
    guard access.allows(port) else {
      outcome("port_tunnel_port_refused")
      return finish(reply(2))
    }
    let base = access.get()
    guard !base.isEmpty, let url = URL(string: base + "&port=\(port)") else {
      outcome("network")
      return finish(reply(1))
    }
    let task = URLSession.shared.webSocketTask(with: URLRequest(url: url, timeoutInterval: 15))
    task.maximumMessageSize = 16 * 1024 * 1024
    tunnel = task
    task.resume()
    // O pong é o primeiro sinal de que o aperto de mão passou; até lá a página não recebe resposta.
    task.sendPing { [weak self] error in
      self?.queue.async {
        guard let self, !self.done else { return }
        if error != nil {
          // O código vem do cabeçalho: o 502 do Caddy com o servidor fora não traz corpo nosso.
          let response = task.response as? HTTPURLResponse
          self.outcome(response?.value(forHTTPHeaderField: "x-hangar-tunnel")
            ?? response.map { "http_\($0.statusCode)" } ?? "network")
          return self.finish(reply(1))
        }
        self.outcome(nil)
        self.connection.send(content: reply(0), completion: .idempotent)
        if !self.buffer.isEmpty {
          let early = Data(self.buffer)
          self.buffer = []
          task.send(.data(self.toServer(early))) { _ in }
        }
        self.pumpUp(task)
        self.pumpDown(task)
      }
    }
  }

  private func toServer(_ data: Data) -> Data {
    if plain == nil { plain = data.first != 0x16 }
    return plain == true ? swapped(data, standIn, localhost) : data
  }

  // Cada sentido só pede o próximo pedaço depois de escrever o anterior: o lado lento freia o outro.
  private func pumpUp(_ task: URLSessionWebSocketTask) {
    connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
      guard let self, !done else { return }
      if error != nil { return close() }
      let next = { [weak self] in
        guard let self, !self.done else { return }
        // Binário vazio = a página parou de mandar, mas espera a resposta.
        if isComplete { task.send(.data(Data())) { _ in } } else { self.pumpUp(task) }
      }
      guard let data, !data.isEmpty else { return next() }
      task.send(.data(toServer(data))) { [weak self] error in
        self?.queue.async {
          if error != nil { self?.close() } else { next() }
        }
      }
    }
  }

  private func pumpDown(_ task: URLSessionWebSocketTask) {
    task.receive { [weak self] result in
      self?.queue.async {
        guard let self, !self.done else { return }
        // Fim do túnel: entrega o que a página ainda não leu e fecha.
        guard case .success(let message) = result else { return self.finish(nil) }
        guard case .data(let data) = message, !data.isEmpty else { return self.pumpDown(task) }
        let out = self.plain == true ? swapped(data, localhost, standIn) : data
        self.connection.send(content: out, completion: .contentProcessed { [weak self] error in
          if error != nil { self?.close() } else { self?.pumpDown(task) }
        })
      }
    }
  }
}
