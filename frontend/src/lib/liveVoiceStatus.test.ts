import { describe, expect, it } from 'vitest';
import { settledSpeaker, speaker, thoughtTail, voiceStatus } from './liveVoiceStatus';

describe('liveVoiceStatus', () => {
  it('who speaks comes from the levels, and muting hides the microphone', () => {
    expect(speaker(0.3, 0.1, false)).toBe('you');
    expect(speaker(0.05, 0.4, false)).toBe('voice');
    expect(speaker(0.3, 0.05, true)).toBe('idle');
    expect(speaker(0.02, 0.03, false)).toBe('idle');
  });

  it('the speaker label holds 300 ms before switching', () => {
    const [still, since] = settledSpeaker('idle', 0, 'you', 100);
    expect(still).toBe('idle');
    expect(settledSpeaker(still, since, 'you', 350)[0]).toBe('you');
    expect(settledSpeaker('you', 0, 'you', 50)).toEqual(['you', 50]);
  });

  it('status follows speech first, then mute, then what the voice is doing', () => {
    expect(voiceStatus(false, 'idle', false, 'idle')).toBe('connecting');
    expect(voiceStatus(true, 'voice', true, 'thinking')).toBe('voice');
    expect(voiceStatus(true, 'idle', true, 'thinking')).toBe('muted');
    expect(voiceStatus(true, 'you', false, 'working')).toBe('you');
    expect(voiceStatus(true, 'idle', false, 'searching')).toBe('searching');
    expect(voiceStatus(true, 'idle', false, 'idle')).toBe('listening');
    expect(voiceStatus(true, 'idle', false, undefined)).toBe('listening');
  });

  it('thought tail keeps the last lines without bold marks', () => {
    expect(thoughtTail('**Plano**\n\nlinha 1\nlinha 2\n**Conferindo**', 3)).toEqual(['linha 1', 'linha 2', 'Conferindo']);
    expect(thoughtTail('')).toEqual([]);
  });
});
