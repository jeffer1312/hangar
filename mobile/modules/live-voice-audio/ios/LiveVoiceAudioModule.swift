import AVFoundation
import ExpoModulesCore
import UIKit
import WebRTC

private struct LiveVoiceAudioOptions: Record {
  @Field var title: String = ""
  @Field var body: String = ""
  @Field var stopLabel: String = ""
}

public class LiveVoiceAudioModule: Module {
  private struct Snapshot {
    let category: String
    let mode: String
    let options: AVAudioSession.CategoryOptions
    let rtcConfiguration: RTCAudioSessionConfiguration
  }

  private var snapshot: Snapshot?
  private var ownsActivation = false
  private var interrupted = false
  private var observers: [NSObjectProtocol] = []
  private var clock: DispatchSourceTimer?
  private var generation = 0


  public func definition() -> ModuleDefinition {
    Name("LiveVoiceAudio")
    Events("interruption", "tick")

    AsyncFunction("start") { (options: LiveVoiceAudioOptions) in
      guard !options.title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            !options.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            !options.stopLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
        throw Exception(name: "audio_labels", description: "Informe os textos da notificação da chamada.", code: "audio_labels")
      }
      try self.startAudio()
    }.runOnQueue(.main)

    AsyncFunction("stop") {
      HangarAudioDevice.shared.held.reset()
      try self.stopAudio()
    }.runOnQueue(.main)

    Function("speechPauseAvailable") { HangarAudioDevice.shared.installed }

    Function("setSpeechPaused") { (paused: Bool) -> Bool in
      let held = HangarAudioDevice.shared.held
      held.setPaused(paused)
      return !paused && held.takeLost()
    }

    OnCreate {
      HangarAudioDevice.shared.onAudioLost = { [weak self] in
        DispatchQueue.main.async { self?.emitInterruption() }
      }
    }

    OnDestroy {
      DispatchQueue.main.async {
        do {
          try self.stopAudio()
        } catch {
          NSLog("LiveVoiceAudio: falha ao restaurar o áudio: %@", error.localizedDescription)
        }
      }
    }
  }

  private func startAudio() throws {
    let rtc = RTCAudioSession.sharedInstance()
    guard rtc.session.recordPermission == .granted else {
      throw Exception(name: "microphone_permission", description: "Permita o microfone para iniciar a chamada.", code: "microphone_permission")
    }
    let starting = snapshot == nil
    if starting && UIApplication.shared.applicationState != .active {
      throw Exception(name: "app_not_visible", description: "Abra o app antes de iniciar a chamada.", code: "app_not_visible")
    }
    if !starting && interrupted {
      throw Exception(name: "audio_interrupted", description: "A sessão de áudio da chamada foi interrompida.", code: "audio_interrupted")
    }

    rtc.lockForConfiguration()
    defer { rtc.unlockForConfiguration() }
    if starting {
      snapshot = Snapshot(
        category: rtc.category,
        mode: rtc.mode,
        options: rtc.categoryOptions,
        rtcConfiguration: RTCAudioSessionConfiguration.webRTC()
      )
      interrupted = false
      // Recarregar o JS no meio da pausa não passa pelo `stop`, e a próxima chamada nasceria muda.
      HangarAudioDevice.shared.held.reset()
    }
    do {
      let configuration = RTCAudioSessionConfiguration()
      configuration.category = AVAudioSession.Category.playAndRecord.rawValue
      configuration.mode = AVAudioSession.Mode.voiceChat.rawValue
      configuration.categoryOptions = [.allowBluetooth, .defaultToSpeaker]
      // Só o áudio padrão do WebRTC relê esta configuração; com o nosso, as preferências vão direto abaixo.
      RTCAudioSessionConfiguration.setWebRTC(configuration)
      try rtc.setCategory(
        AVAudioSession.Category(rawValue: configuration.category),
        mode: AVAudioSession.Mode(rawValue: configuration.mode),
        options: configuration.categoryOptions
      )
      try? rtc.session.setPreferredSampleRate(configuration.sampleRate)
      try? rtc.session.setPreferredIOBufferDuration(configuration.ioBufferDuration)
      // Gravando, o iOS cala a vibração, e ela é o aviso de que a chamada conectou.
      try? rtc.session.setAllowHapticsAndSystemSoundsDuringRecording(true)
      if !ownsActivation {
        try rtc.setActive(true)
        ownsActivation = true
      }
      if starting {
        observeInterruptions()
        startClock()
      }
    } catch {
      if starting {
        do {
          try restoreAudio(rtc)
        } catch let restoreError {
          throw Exception(name: "audio_start", description: "\(error.localizedDescription) Restauração: \(restoreError.localizedDescription)", code: "audio_start")
        }
      } else {
        emitInterruption()
      }
      throw error
    }
  }

  private func startClock() {
    generation += 1
    let token = generation
    let timer = DispatchSource.makeTimerSource(queue: .main)
    timer.schedule(deadline: .now() + .milliseconds(100), repeating: .milliseconds(100))
    timer.setEventHandler { [weak self] in
      guard let self, self.generation == token, self.snapshot != nil, !self.interrupted else { return }
      self.sendEvent("tick", [:])
    }
    clock = timer
    timer.resume()
  }

  private func observeInterruptions() {
    let center = NotificationCenter.default
    observers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] notification in
      guard let value = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            AVAudioSession.InterruptionType(rawValue: value) == .began else { return }
      self?.emitInterruption()
    })
    for name in [AVAudioSession.mediaServicesWereLostNotification, AVAudioSession.mediaServicesWereResetNotification] {
      observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
        self?.emitInterruption()
      })
    }
  }

  private func emitInterruption() {
    guard snapshot != nil && !interrupted else { return }
    interrupted = true
    sendEvent("interruption", ["reason": "audio_interrupted"])
  }

  private func stopAudio() throws {
    guard snapshot != nil else { return }
    let rtc = RTCAudioSession.sharedInstance()
    rtc.lockForConfiguration()
    defer { rtc.unlockForConfiguration() }
    try restoreAudio(rtc)
  }

  private func restoreAudio(_ rtc: RTCAudioSession) throws {
    guard let previous = snapshot else { return }
    snapshot = nil
    interrupted = false
    generation += 1
    clock?.cancel()
    clock = nil
    observers.forEach { NotificationCenter.default.removeObserver($0) }
    observers.removeAll()
    RTCAudioSessionConfiguration.setWebRTC(previous.rtcConfiguration)
    try? rtc.session.setAllowHapticsAndSystemSoundsDuringRecording(false)
    var errors: [String] = []
    if ownsActivation {
      ownsActivation = false
      // A ativação é contada pelo RTC; parar duas vezes não desativa outro áudio.
      do { try rtc.setActive(false) } catch { errors.append(error.localizedDescription) }
    }
    do {
      try rtc.setCategory(
        AVAudioSession.Category(rawValue: previous.category),
        mode: AVAudioSession.Mode(rawValue: previous.mode),
        options: previous.options
      )
    } catch {
      errors.append(error.localizedDescription)
    }
    if !errors.isEmpty {
      throw Exception(name: "audio_restore", description: errors.joined(separator: "; "), code: "audio_restore")
    }
  }
}
