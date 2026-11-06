import { NativeModule, requireNativeModule } from 'expo';

export type LiveVoiceAudioOptions = {
  title: string;
  body: string;
  stopLabel: string;
};

export type LiveVoiceAudioEvents = {
  interruption: (event: { reason: string }) => void;
  tick: () => void;
};

declare class LiveVoiceAudioModule extends NativeModule<LiveVoiceAudioEvents> {
  start(options: LiveVoiceAudioOptions): Promise<void>;
  stop(): Promise<void>;
  /** Só no iPhone, e só em binário que traz o dispositivo de áudio próprio. */
  speechPauseAvailable?(): boolean;
  setSpeechPaused?(paused: boolean): void;
}

export default requireNativeModule<LiveVoiceAudioModule>('LiveVoiceAudio');
