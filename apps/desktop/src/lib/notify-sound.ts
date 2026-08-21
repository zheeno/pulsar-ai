const STORAGE_KEY = 'pulsar.notificationSounds';

export type NotifySoundKind = 'success' | 'info' | 'warning' | 'error';

let ctx: AudioContext | null = null;
let lastPlayAt = 0;
const MIN_GAP_MS = 280;

export function notificationSoundsEnabled(): boolean {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return true;
    return raw === '1' || raw === 'true';
  } catch {
    return true;
  }
}

export function setNotificationSoundsEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(STORAGE_KEY, enabled ? '1' : '0');
  } catch {
    /* ignore */
  }
}

function audioContext(): AudioContext | null {
  if (typeof window === 'undefined') return null;
  const AC =
    window.AudioContext ||
    (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!AC) return null;
  if (!ctx) ctx = new AC();
  return ctx;
}

function tone(
  audio: AudioContext,
  freq: number,
  start: number,
  duration: number,
  gain: number,
  type: OscillatorType = 'sine',
): void {
  const osc = audio.createOscillator();
  const g = audio.createGain();
  osc.type = type;
  osc.frequency.setValueAtTime(freq, start);
  g.gain.setValueAtTime(0.0001, start);
  g.gain.exponentialRampToValueAtTime(gain, start + 0.018);
  g.gain.exponentialRampToValueAtTime(0.0001, start + duration);
  osc.connect(g);
  g.connect(audio.destination);
  osc.start(start);
  osc.stop(start + duration + 0.02);
}

/** Short synthesized cues — no asset files required. */
export function playNotificationSound(kind: NotifySoundKind): void {
  if (!notificationSoundsEnabled()) return;
  const now = Date.now();
  if (now - lastPlayAt < MIN_GAP_MS) return;
  lastPlayAt = now;

  const audio = audioContext();
  if (!audio) return;

  const resume = audio.state === 'suspended' ? audio.resume() : Promise.resolve();
  void resume.then(() => {
    const t0 = audio.currentTime + 0.01;
    switch (kind) {
      case 'success':
        tone(audio, 660, t0, 0.09, 0.07);
        tone(audio, 880, t0 + 0.1, 0.12, 0.08);
        break;
      case 'info':
        tone(audio, 520, t0, 0.1, 0.045);
        break;
      case 'warning':
        tone(audio, 440, t0, 0.12, 0.09, 'triangle');
        tone(audio, 440, t0 + 0.16, 0.14, 0.1, 'triangle');
        break;
      case 'error':
        tone(audio, 320, t0, 0.14, 0.11, 'square');
        tone(audio, 240, t0 + 0.15, 0.18, 0.12, 'square');
        break;
      default:
        break;
    }
  });
}

export function shouldPlaySoundForToast(kind: NotifySoundKind, explicit?: boolean): boolean {
  if (explicit === false) return false;
  if (explicit === true) return true;
  // Info toasts are often progress chatter ("Saving…"); keep those silent by default.
  return kind !== 'info';
}
