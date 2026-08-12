import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { listen } from '@tauri-apps/api/event';
import { api } from './api';
import { useToast } from './toast';

type CycleCompletePayload = {
  ok?: boolean;
  source?: string;
  signals?: number;
  executed?: number;
  warnings?: string[];
  error?: string;
};

type CycleContextValue = {
  running: boolean;
};

const CycleContext = createContext<CycleContextValue>({ running: false });

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export function CycleProvider({ children }: { children: ReactNode }) {
  const toast = useToast();
  const [running, setRunning] = useState(false);

  const onComplete = useCallback(
    (payload: CycleCompletePayload) => {
      setRunning(false);
      const source = payload.source === 'scheduler' ? 'Auto cycle' : 'Cycle';
      if (payload.ok === false) {
        toast.error(payload.error || 'Trading cycle failed.', source);
        return;
      }
      const signals = payload.signals ?? 0;
      const executed = payload.executed ?? 0;
      const warnings = payload.warnings?.length
        ? ` Warnings: ${payload.warnings.join('; ')}`
        : '';
      toast.success(`${signals} signals, ${executed} executed.${warnings}`, `${source} complete`);
    },
    [toast],
  );

  useEffect(() => {
    let cancelled = false;
    const unlisteners: Array<() => void> = [];

    async function boot() {
      try {
        const status = await api<{ running: boolean }>('cycle_status');
        if (!cancelled) setRunning(!!status.running);
      } catch {
        /* ignore */
      }

      if (!isTauri()) return;

      try {
        const u1 = await listen<{ source?: string }>('cycle:start', () => {
          if (!cancelled) setRunning(true);
        });
        unlisteners.push(u1);

        const u2 = await listen<CycleCompletePayload>('cycle:complete', (event) => {
          if (!cancelled) onComplete(event.payload || {});
        });
        unlisteners.push(u2);
      } catch {
        /* event API unavailable */
      }
    }

    void boot();
    return () => {
      cancelled = true;
      for (const u of unlisteners) u();
    };
  }, [onComplete]);

  const value = useMemo(() => ({ running }), [running]);

  return <CycleContext.Provider value={value}>{children}</CycleContext.Provider>;
}

export function useCycle(): CycleContextValue {
  return useContext(CycleContext);
}
