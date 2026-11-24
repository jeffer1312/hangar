package expo.modules.porttunnel

import java.io.DataInputStream
import java.io.IOException
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.Collections
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString

private const val HANDSHAKE_TIMEOUT_MS = 10_000
private const val OPEN_TIMEOUT_S = 15L
private const val SEND_HIGH_WATER = 1024L * 1024
// O navegador abre umas seis conexões por host; sobra para várias portas e o HMR.
private const val MAX_CONNECTIONS = 64

// OkHttp responde aos pings do servidor sozinho.
private val client = OkHttpClient.Builder().readTimeout(0, TimeUnit.MILLISECONDS).build()

private fun isLoopbackName(name: String): Boolean {
  val host = name.lowercase().removeSuffix(".").removePrefix("[").removeSuffix("]")
  if (host == "localhost" || host.endsWith(".localhost")) return true
  val literal = host.isNotEmpty() && host.all { it.isDigit() || it == '.' || it == ':' || it in 'a'..'f' }
  return literal && runCatching { InetAddress.getByName(host).isLoopbackAddress }.getOrDefault(false)
}

/**
 * SOCKS (5 e 4/4a) em 127.0.0.1 que leva cada CONNECT ao loopback por um WebSocket do `/api/port-tunnel`.
 * Nunca conecta a nada deste aparelho: túnel que falha é erro, nunca um servidor do próprio celular.
 * Vive só enquanto há página do túnel na tela.
 */
// Sem senha (o WebView não autentica SOCKS): outro app do aparelho alcança o SOCKS, então só as portas da sessão passam.
internal class SocksListener(private val access: () -> String, private val allowed: () -> Set<Int>) {
  private val server = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
  private val sockets = Collections.newSetFromMap(ConcurrentHashMap<Socket, Boolean>())
  val port: Int = server.localPort

  /** Código da última recusa do túnel, ou null depois de um túnel que abriu. */
  @Volatile var lastFailure: String? = null
    private set

  init {
    Thread({
      while (!server.isClosed) {
        val socket = runCatching { server.accept() }.getOrNull() ?: break
        val admitted = synchronized(sockets) { !server.isClosed && sockets.size < MAX_CONNECTIONS && sockets.add(socket) }
        if (admitted) Thread({ serve(socket) }, "hangar-port-tunnel").apply { isDaemon = true }.start()
        else runCatching { socket.close() }
      }
    }, "hangar-port-tunnel-accept").apply { isDaemon = true }.start()
  }

  fun close() {
    val open = synchronized(sockets) {
      runCatching { server.close() }
      sockets.toList().also { sockets.clear() }
    }
    open.forEach { runCatching { it.close() } }
  }

  private class Connect(val port: Int, val reply: (Boolean) -> ByteArray)

  private fun serve(socket: Socket) {
    try {
      socket.soTimeout = HANDSHAKE_TIMEOUT_MS
      val input = DataInputStream(socket.getInputStream())
      val output = socket.getOutputStream()
      val request = when (input.readUnsignedByte()) {
        5 -> readSocks5(input, output)
        4 -> readSocks4(input, output)
        else -> null
      }
      if (request != null) tunnel(socket, input, output, request)
    } catch (_: Exception) {
    } finally {
      synchronized(sockets) { sockets.remove(socket) }
      runCatching { socket.close() }
    }
  }

  /** null = já respondeu a recusa. */
  private fun readSocks5(input: DataInputStream, output: OutputStream): Connect? {
    val methods = ByteArray(input.readUnsignedByte()).also { input.readFully(it) }
    if (!methods.contains(0)) {
      output.write(byteArrayOf(5, 0xff.toByte()))
      return null
    }
    output.write(byteArrayOf(5, 0))
    val version = input.readUnsignedByte()
    val command = input.readUnsignedByte()
    input.readUnsignedByte()
    val loopback = when (input.readUnsignedByte()) {
      1 -> ByteArray(4).also { input.readFully(it) }[0] == 127.toByte()
      4 -> InetAddress.getByAddress(ByteArray(16).also { input.readFully(it) }).isLoopbackAddress
      3 -> isLoopbackName(String(ByteArray(input.readUnsignedByte()).also { input.readFully(it) }))
      else -> return refuse(output, socks5(8))
    }
    val port = input.readUnsignedShort()
    if (version != 5 || command != 1) return refuse(output, socks5(7))
    if (!loopback) return refuse(output, socks5(2))
    return Connect(port) { ok -> socks5(if (ok) 0 else 1) }
  }

  private fun readSocks4(input: DataInputStream, output: OutputStream): Connect? {
    val command = input.readUnsignedByte()
    val port = input.readUnsignedShort()
    val address = ByteArray(4).also { input.readFully(it) }
    cString(input)
    // SOCKS4a: 0.0.0.x quer dizer que o nome vem depois do usuário.
    val named = address[0] == 0.toByte() && address[1] == 0.toByte() && address[2] == 0.toByte() && address[3] != 0.toByte()
    val loopback = if (named) isLoopbackName(cString(input)) else address[0] == 127.toByte()
    val refused = byteArrayOf(0, 0x5b, 0, 0, 0, 0, 0, 0)
    if (command != 1 || !loopback) return refuse(output, refused)
    return Connect(port) { ok -> if (ok) byteArrayOf(0, 0x5a, 0, 0, 0, 0, 0, 0) else refused }
  }

  private fun refuse(output: OutputStream, reply: ByteArray): Connect? {
    output.write(reply)
    return null
  }

  private fun cString(input: DataInputStream): String {
    val text = StringBuilder()
    while (text.length <= 255) {
      val next = input.readUnsignedByte()
      if (next == 0) return text.toString()
      text.append(next.toChar())
    }
    throw IOException("SOCKS4 string too long")
  }

  private fun socks5(code: Int) = byteArrayOf(5, code.toByte(), 0, 1, 0, 0, 0, 0, 0, 0)

  private fun tunnel(socket: Socket, input: DataInputStream, output: OutputStream, request: Connect) {
    if (request.port !in allowed()) {
      lastFailure = "port_tunnel_port_refused"
      output.write(request.reply(false))
      return
    }
    val base = access()
    if (base.isEmpty()) {
      lastFailure = "network"
      output.write(request.reply(false))
      return
    }
    val opened = CompletableFuture<Boolean>()
    val replied = CompletableFuture<Unit>()
    val closed = CompletableFuture<Unit>()
    var failure = "network"

    // O servidor pode falar primeiro; os bytes dele não chegam à página antes da resposta SOCKS.
    fun toPage(webSocket: WebSocket, bytes: ByteArray) {
      runCatching {
        replied.get()
        output.write(bytes)
      }.onFailure { webSocket.cancel() }
    }

    val webSocket = client.newWebSocket(
      Request.Builder().url("$base&port=${request.port}").build(),
      object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
          opened.complete(true)
        }

        // Escrever aqui segura a thread de leitura do OkHttp: página que não lê freia o túnel.
        override fun onMessage(webSocket: WebSocket, bytes: ByteString) = toPage(webSocket, bytes.toByteArray())

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
          webSocket.close(1000, null)
          closed.complete(Unit)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
          if (!opened.isDone) {
            // O código vem do cabeçalho: o 502 do Caddy com o servidor fora não traz corpo nosso.
            failure = response?.header("x-hangar-tunnel") ?: response?.let { "http_${it.code}" } ?: "network"
          }
          opened.complete(false)
          closed.complete(Unit)
        }
      },
    )
    if (!runCatching { opened.get(OPEN_TIMEOUT_S, TimeUnit.SECONDS) }.getOrDefault(false)) {
      lastFailure = failure
      replied.completeExceptionally(IOException("tunnel did not open"))
      webSocket.cancel()
      output.write(request.reply(false))
      return
    }
    if (runCatching { output.write(request.reply(true)) }.isFailure) {
      replied.completeExceptionally(IOException("page went away"))
      webSocket.cancel()
      return
    }
    lastFailure = null
    replied.complete(Unit)
    socket.soTimeout = 0
    val up = Thread({ fromPage(input, webSocket, closed) }, "hangar-port-tunnel-up").apply { isDaemon = true }
    up.start()
    closed.get()
    runCatching {
      output.flush()
      socket.shutdownOutput()
    }
    up.join(1000)
  }

  private fun fromPage(input: DataInputStream, webSocket: WebSocket, closed: CompletableFuture<Unit>) {
    val buffer = ByteArray(64 * 1024)
    try {
      var count = input.read(buffer)
      while (count >= 0) {
        webSocket.send(buffer.toByteString(0, count))
        // O OkHttp enfileira sem limite e derruba o socket passando de 16 MiB.
        while (webSocket.queueSize() > SEND_HIGH_WATER && !closed.isDone) Thread.sleep(5)
        count = input.read(buffer)
      }
      // Binário vazio = a página parou de mandar, mas espera a resposta.
      webSocket.send(ByteString.EMPTY)
    } catch (_: Exception) {
      webSocket.cancel()
    }
  }
}
