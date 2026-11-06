import AVFoundation
import ExpoModulesCore
import WebRTC

/// Fala guardada na pausa. O modelo segue gerando em tempo real, então o áudio continua sendo puxado do WebRTC, que o
/// descartaria, e toca depois, na ordem. Só a thread de áudio mexe no anel; quem pede pausa e limpeza usa as bandeiras.
final class HeldSpeech {
  // Teto de 10 min de fala em 48 kHz; a memória só é ocupada conforme o anel enche.
  private static let capacity = 48_000 * 600
  // Abaixo disto é silêncio: guardá-lo deixaria a voz atrasada para sempre, porque o WebRTC entrega silêncio sem parar.
  private static let silence: Int16 = 8

  private let samples = UnsafeMutablePointer<Int16>.allocate(capacity: HeldSpeech.capacity)
  private let flags = UnsafeMutablePointer<Int32>.allocate(capacity: 2)
  private var start = 0
  private var count = 0

  init() { flags.initialize(repeating: 0, count: 2) }

  deinit {
    samples.deallocate()
    flags.deallocate()
  }

  func setPaused(_ paused: Bool) { flags[0] = paused ? 1 : 0 }

  func reset() {
    flags[0] = 0
    flags[1] = 1
  }

  /// A taxa do áudio mudou: o que está guardado tocaria na velocidade errada.
  func discard() { flags[1] = 1 }

  func process(_ out: UnsafeMutablePointer<Int16>, _ frames: Int) {
    if flags[1] != 0 {
      flags[1] = 0
      (start, count) = (0, 0)
    }
    let paused = flags[0] != 0
    if !paused && count == 0 { return }
    if !isSilent(out, frames) { write(out, frames) }
    if paused {
      out.update(repeating: 0, count: frames)
      return
    }
    read(out, frames)
  }

  private func isSilent(_ out: UnsafeMutablePointer<Int16>, _ frames: Int) -> Bool {
    for index in 0..<frames where out[index] > HeldSpeech.silence || out[index] < -HeldSpeech.silence { return false }
    return true
  }

  private func write(_ out: UnsafeMutablePointer<Int16>, _ frames: Int) {
    let room = min(frames, HeldSpeech.capacity - count)
    for index in 0..<room { samples[(start + count + index) % HeldSpeech.capacity] = out[index] }
    count += room
  }

  private func read(_ out: UnsafeMutablePointer<Int16>, _ frames: Int) {
    let taken = min(frames, count)
    for index in 0..<taken { out[index] = samples[(start + index) % HeldSpeech.capacity] }
    if taken < frames { (out + taken).update(repeating: 0, count: frames - taken) }
    start = (start + taken) % HeldSpeech.capacity
    count -= taken
  }
}

/// Áudio do WebRTC pelo nosso lado: o padrão toca dentro do motor nativo e não deixa guardar a fala.
/// Adaptado de github.com/mstyura/RTCAudioDevice (MIT).
final class HangarAudioDevice: NSObject {
  static let shared = HangarAudioDevice()

  let held = HeldSpeech()
  private(set) var installed = false

  private let session = AVAudioSession.sharedInstance()
  private var audioUnit: AUAudioUnit?
  private var delegate: RTCAudioDeviceDelegate?
  private var routeObserver: NSObjectProtocol?
  private var resetObserver: NSObjectProtocol?
  private var shouldPlay = false
  private var shouldRecord = false

  /// Antes de o módulo do WebRTC nascer: ele lê a opção uma vez só. O nome da classe evita depender do pod dele.
  func install() {
    guard let type = NSClassFromString("WebRTCModuleOptions") as? NSObject.Type,
          let options = type.perform(NSSelectorFromString("sharedInstance"))?.takeUnretainedValue() as? NSObject else {
      NSLog("LiveVoiceAudio: WebRTCModuleOptions indisponível; a pausa da fala fica desligada")
      return
    }
    options.setValue(self, forKey: "audioDevice")
    installed = true
  }

  private func makeUnit() -> AUAudioUnit? {
    // VoiceProcessingIO: o mesmo cancelamento de eco do áudio padrão do WebRTC no iPhone.
    let description = AudioComponentDescription(componentType: kAudioUnitType_Output, componentSubType: kAudioUnitSubType_VoiceProcessingIO,
      componentManufacturer: kAudioUnitManufacturer_Apple, componentFlags: 0, componentFlagsMask: 0)
    do {
      let unit = try AUAudioUnit(componentDescription: description)
      unit.isInputEnabled = false
      unit.isOutputEnabled = false
      unit.maximumFramesToRender = 1024
      return unit
    } catch {
      NSLog("LiveVoiceAudio: falha ao criar o áudio: %@", error.localizedDescription)
      return nil
    }
  }

  private func format() -> AVAudioFormat? {
    AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: session.sampleRate, channels: 1, interleaved: true)
  }

  private func stopUnit(_ unit: AUAudioUnit) {
    if unit.isRunning {
      unit.stopHardware()
      delegate?.notifyAudioInputInterrupted()
      delegate?.notifyAudioOutputInterrupted()
    }
    if unit.renderResourcesAllocated { unit.deallocateRenderResources() }
  }

  @discardableResult
  private func updateUnit() -> Bool {
    guard let unit = audioUnit else { return false }
    guard let delegate, shouldPlay || shouldRecord else {
      stopUnit(unit)
      return true
    }
    guard let wanted = format() else { return false }
    if unit.isInputEnabled != shouldRecord || unit.isOutputEnabled != shouldPlay {
      stopUnit(unit)
      unit.isInputEnabled = shouldRecord
      unit.isOutputEnabled = shouldPlay
    }
    do {
      if shouldRecord {
        let bus = unit.outputBusses[1]
        if bus.format != wanted {
          stopUnit(unit)
          try bus.setFormat(wanted)
          // Sem o aviso o WebRTC segue com a taxa antiga, e com fone Bluetooth o microfone sairia acelerado.
          delegate.notifyAudioInputParametersChange()
        }
        let deliver = delegate.deliverRecordedData
        let render = unit.renderBlock
        let recorded: RTCAudioDeviceRenderRecordedDataBlock = { flags, timestamp, busNumber, frames, data, _ in
          render(flags, timestamp, frames, busNumber, data, nil)
        }
        unit.inputHandler = { flags, timestamp, frames, busNumber in
          _ = deliver(flags, timestamp, busNumber, frames, nil, nil, recorded)
        }
      }
      if shouldPlay {
        let bus = unit.inputBusses[0]
        if bus.format != wanted {
          stopUnit(unit)
          if bus.format.sampleRate != wanted.sampleRate {
            NSLog("LiveVoiceAudio: taxa de áudio mudou; a fala guardada foi descartada")
            held.discard()
          }
          try bus.setFormat(wanted)
          delegate.notifyAudioOutputParametersChange()
        }
        if unit.outputProvider == nil {
          let playout = delegate.getPlayoutData
          let held = self.held
          unit.outputProvider = { flags, timestamp, frames, busNumber, data in
            let status = playout(flags, timestamp, busNumber, frames, data)
            if status == noErr, let raw = UnsafeMutableAudioBufferListPointer(data)[0].mData {
              held.process(raw.assumingMemoryBound(to: Int16.self), Int(frames))
            }
            return status
          }
        }
      }
      if !unit.renderResourcesAllocated { try unit.allocateRenderResources() }
      if !unit.isRunning { try unit.startHardware() }
      return true
    } catch {
      NSLog("LiveVoiceAudio: falha ao ligar o áudio: %@", error.localizedDescription)
      return false
    }
  }
}

extension HangarAudioDevice: RTCAudioDevice {
  var deviceInputSampleRate: Double { session.sampleRate }
  var inputIOBufferDuration: TimeInterval { session.ioBufferDuration }
  var inputNumberOfChannels: Int { 1 }
  var inputLatency: TimeInterval { session.inputLatency }
  var deviceOutputSampleRate: Double { session.sampleRate }
  var outputIOBufferDuration: TimeInterval { session.ioBufferDuration }
  var outputNumberOfChannels: Int { 1 }
  var outputLatency: TimeInterval { session.outputLatency }
  var isInitialized: Bool { delegate != nil && audioUnit != nil }

  func initialize(with delegate: RTCAudioDeviceDelegate) -> Bool {
    guard self.delegate == nil, let unit = makeUnit() else { return false }
    audioUnit = unit
    self.delegate = delegate
    let center = NotificationCenter.default
    routeObserver = center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: nil) { [weak self] _ in
      self?.delegate?.dispatchAsync { self?.updateUnit() }
    }
    // A unidade nasce uma vez por processo: morta pelo reset do áudio, as chamadas seguintes ficariam mudas.
    resetObserver = center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: nil) { [weak self] _ in
      self?.delegate?.dispatchAsync {
        guard let self else { return }
        if let old = self.audioUnit { self.stopUnit(old) }
        self.audioUnit = self.makeUnit()
        self.updateUnit()
      }
    }
    return true
  }

  func terminateDevice() -> Bool {
    for observer in [routeObserver, resetObserver].compactMap({ $0 }) { NotificationCenter.default.removeObserver(observer) }
    (routeObserver, resetObserver) = (nil, nil)
    (shouldPlay, shouldRecord) = (false, false)
    updateUnit()
    audioUnit = nil
    delegate = nil
    return true
  }

  var isPlayoutInitialized: Bool { isInitialized }
  func initializePlayout() -> Bool { isPlayoutInitialized }
  var isPlaying: Bool { shouldPlay }

  func startPlayout() -> Bool {
    shouldPlay = true
    return updateUnit()
  }

  func stopPlayout() -> Bool {
    shouldPlay = false
    return updateUnit()
  }

  var isRecordingInitialized: Bool { isInitialized }
  func initializeRecording() -> Bool { isRecordingInitialized }
  var isRecording: Bool { shouldRecord }

  func startRecording() -> Bool {
    shouldRecord = true
    return updateUnit()
  }

  func stopRecording() -> Bool {
    shouldRecord = false
    return updateUnit()
  }
}

public class LiveVoiceAudioAppDelegate: ExpoAppDelegateSubscriber {
  public func application(_ application: UIApplication, willFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
    HangarAudioDevice.shared.install()
    return true
  }
}
