import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { playNotificationSound, shouldPlaySoundForToast } from './notify-sound';

export type ToastKind = 'success' | 'info' | 'warning' | 'error';

export type ToastInput = {
  kind: ToastKind;
  message: string;
  title?: string;
  durationMs?: number;
  /** Override default sound policy (info silent; success/warning/error play). */
  sound?: boolean;
};

type ToastItem = ToastInput & { id: string };

type ToastOptions = {
  sound?: boolean;
  durationMs?: number;
};

type ToastContextValue = {
  push: (toast: ToastInput) => void;
  success: (message: string, title?: string, opts?: ToastOptions) => void;
  info: (message: string, title?: string, opts?: ToastOptions) => void;
  warning: (message: string, title?: string, opts?: ToastOptions) => void;
  error: (message: string, title?: string, opts?: ToastOptions) => void;
  dismiss: (id: string) => void;
};

const ToastContext = createContext<ToastContextValue | null>(null);

const DEFAULT_DURATION: Record<ToastKind, number> = {
  success: 4000,
  info: 4000,
  warning: 5000,
  error: 6000,
};

function uid() {
  return `t-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timers = useRef<Map<string, number>>(new Map());

  const dismiss = useCallback((id: string) => {
    const timer = timers.current.get(id);
    if (timer) {
      window.clearTimeout(timer);
      timers.current.delete(id);
    }
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const push = useCallback(
    (toast: ToastInput) => {
      const id = uid();
      const item: ToastItem = { ...toast, id };
      setToasts((prev) => [...prev, item].slice(-5));
      if (shouldPlaySoundForToast(toast.kind, toast.sound)) {
        playNotificationSound(toast.kind);
      }
      const duration = toast.durationMs ?? DEFAULT_DURATION[toast.kind];
      const timer = window.setTimeout(() => dismiss(id), duration);
      timers.current.set(id, timer);
    },
    [dismiss],
  );

  const value = useMemo<ToastContextValue>(
    () => ({
      push,
      dismiss,
      success: (message, title, opts) =>
        push({ kind: 'success', message, title, sound: opts?.sound, durationMs: opts?.durationMs }),
      info: (message, title, opts) =>
        push({ kind: 'info', message, title, sound: opts?.sound, durationMs: opts?.durationMs }),
      warning: (message, title, opts) =>
        push({ kind: 'warning', message, title, sound: opts?.sound, durationMs: opts?.durationMs }),
      error: (message, title, opts) =>
        push({ kind: 'error', message, title, sound: opts?.sound, durationMs: opts?.durationMs }),
    }),
    [push, dismiss],
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-host" aria-live="polite" aria-relevant="additions">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast--${t.kind}`} role="status">
            <div className="toast__body">
              {t.title ? <div className="toast__title">{t.title}</div> : null}
              <div className="toast__message">{t.message}</div>
            </div>
            <button
              type="button"
              className="toast__close"
              aria-label="Dismiss"
              onClick={() => dismiss(t.id)}
            >
              ×
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) {
    throw new Error('useToast must be used within ToastProvider');
  }
  return ctx;
}
