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
  universeSize?: number;
  instrumentsIngested?: number;
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
      const universe = payload.universeSize != null ? ` Universe ${payload.universeSize}.` : '';
      const warnings = payload.warnings?.length
        ? ` Warnings: ${payload.warnings.join('; ')}`
        : '';
      toast.success(`${signals} signals, ${executed} executed.${universe}${warnings}`, `${source} complete`);
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

        const u3 = await listen<{
          symbols?: string[];
          label?: string;
          executed?: number;
          exits?: number;
          executedLive?: boolean;
          tradingMode?: string;
          warnings?: string[];
        }>('risk-exit', (event) => {
          if (cancelled) return;
          const p = event.payload || {};
          const symbols = (p.symbols ?? []).filter(Boolean);
          const label = p.label || 'Risk exit';
          const names = symbols.length ? symbols.join(', ') : 'position';
          const executed = p.executed ?? 0;
          if (executed > 0) {
            toast.success(`${label} sold ${names}`, 'Risk exit');
          } else if ((p.exits ?? 0) > 0) {
            const warn = Array.isArray(p.warnings) && p.warnings.length
              ? String(p.warnings[0])
              : '';
            const detail = warn
              || (p.executedLive === false
                ? (p.tradingMode === 'sandbox'
                  ? 'broker not live — signal saved only'
                  : 'venue closed or live submit skipped')
                : 'submit did not fill yet — check Signals / logs');
            toast.warning(`${label} for ${names}: ${detail}`, 'Risk exit', { sound: true });
          }
        });
        unlisteners.push(u3);
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

  // Recover if we saw a stale `running` snapshot (risk/dream used to share the
  // cycle flag) or missed `cycle:complete` after a tauri reload.
  useEffect(() => {
    if (!running) return;
    let cancelled = false;
    const tick = window.setInterval(() => {
      void api<{ running: boolean }>('cycle_status')
        .then((status) => {
          if (!cancelled && !status.running) setRunning(false);
        })
        .catch(() => {
          /* keep current */
        });
    }, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(tick);
    };
  }, [running]);

  const value = useMemo(() => ({ running }), [running]);

  return <CycleContext.Provider value={value}>{children}</CycleContext.Provider>;
}

export function useCycle(): CycleContextValue {
  return useContext(CycleContext);
}
