import { useCallback, useEffect, useMemo, useState } from 'react';
import { HashRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import AppShell from './components/AppShell';
import AuthBridgeSessionAlerts from './components/AuthBridgeSessionAlerts';
import ErrorBoundary from './components/ErrorBoundary';
import SplashScreen from './components/SplashScreen';
import { api, isTauri, type AppSettings } from './lib/api';
import { SessionContext } from './lib/session';
import { ToastProvider } from './lib/toast';
import { CycleProvider } from './lib/cycle';
import Dashboard from './pages/Dashboard';
import Signals from './pages/Signals';
import Trades from './pages/Trades';
import Memory from './pages/Memory';
import Coach from './pages/Coach';
import Settings from './pages/Settings';
import Onboarding from './pages/Onboarding';
import SymbolDetail from './pages/SymbolDetail';

type PulseAuthReport = {
  ok: boolean;
  authMode: string;
  message: string;
};

const BOOT_TIMEOUT_MS = 12_000;
const BOOT_TIMEOUT_MESSAGE = 'Startup timed out while contacting the desktop backend.';
const SPLASH_SEEN_KEY = 'pulsar.splash.seen';

function hasSeenSplash(): boolean {
  try {
    return sessionStorage.getItem(SPLASH_SEEN_KEY) === '1';
  } catch {
    return false;
  }
}

function markSplashSeen() {
  try {
    sessionStorage.setItem(SPLASH_SEEN_KEY, '1');
  } catch {
    /* private mode */
  }
}

function withBootTimeout<T>(promise: Promise<T>): Promise<T> {
  return Promise.race([
    promise,
    new Promise<never>((_, reject) => {
      window.setTimeout(() => reject(new Error(BOOT_TIMEOUT_MESSAGE)), BOOT_TIMEOUT_MS);
    }),
  ]);
}

function AppRoutes() {
  const navigate = useNavigate();
  const [ready, setReady] = useState(false);
  const [splashDone, setSplashDone] = useState(hasSeenSplash);
  const [needsOnboarding, setNeedsOnboarding] = useState(true);
  const [bootMessage, setBootMessage] = useState('Igniting Pulsar…');
  const [bootError, setBootError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      try {
        const s = await withBootTimeout(api<AppSettings>('settings_get'));
        if (cancelled) return;

        if (!s.onboardingComplete) {
          setNeedsOnboarding(true);
          setReady(true);
          return;
        }

        setBootMessage('Verifying NGX Pulse session…');
        const report = await withBootTimeout(api<PulseAuthReport>('test_pulse_login'));
        if (cancelled) return;

        if (!report.ok || report.authMode !== 'session') {
          setBootMessage('Pulse session invalid — signing out…');
          try {
            await api('logout');
          } catch {
            /* still force onboarding */
          }
          if (cancelled) return;
          setNeedsOnboarding(true);
          setReady(true);
          navigate('/onboarding', { replace: true });
          return;
        }

        setBootMessage('Ready');
        setNeedsOnboarding(false);
        setReady(true);
      } catch (err) {
        if (cancelled) return;
        const message =
          err instanceof Error ? err.message : 'Could not reach the desktop backend.';
        setBootError(message);
        setBootMessage('Startup issue — opening setup…');
        setNeedsOnboarding(true);
        setReady(true);
      }
    }

    void boot();
    return () => {
      cancelled = true;
    };
  }, [navigate]);

  const markOnboardingComplete = useCallback(() => {
    setNeedsOnboarding(false);
  }, []);

  const logout = useCallback(async () => {
    await api('logout');
    setNeedsOnboarding(true);
    navigate('/onboarding', { replace: true });
  }, [navigate]);

  const session = useMemo(() => ({ logout }), [logout]);
  const finishSplash = useCallback(() => {
    markSplashSeen();
    setSplashDone(true);
  }, []);

  if (!splashDone) {
    return (
      <SplashScreen
        ready={ready}
        message={bootError ? `${bootMessage} ${bootError}` : bootMessage}
        onFinished={finishSplash}
      />
    );
  }

  return (
    <SessionContext.Provider value={session}>
      <Routes>
        <Route
          path="/onboarding"
          element={
            needsOnboarding ? (
              <Onboarding onComplete={markOnboardingComplete} />
            ) : (
              <Navigate to="/" replace />
            )
          }
        />
        <Route
          element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <AppShell />}
        >
          <Route path="/" element={<Dashboard />} />
          <Route path="/signals" element={<Signals />} />
          <Route path="/trades" element={<Trades />} />
          <Route path="/symbol/:symbol" element={<SymbolDetail />} />
          <Route path="/memory" element={<Memory />} />
          <Route path="/coach" element={<Coach />} />
          <Route path="/settings" element={<Settings />} />
        </Route>
      </Routes>
    </SessionContext.Provider>
  );
}

export default function App() {
  const browserMode = typeof window !== 'undefined' && !isTauri();

  return (
    <HashRouter>
      <ErrorBoundary>
        <ToastProvider>
          <AuthBridgeSessionAlerts />
          <CycleProvider>
            <div className="app-root">
              {browserMode && (
                <div className="browser-banner">
                  Browser mock mode — sample data in localStorage. Pulse gating requires the desktop app.
                </div>
              )}
              <AppRoutes />
            </div>
          </CycleProvider>
        </ToastProvider>
      </ErrorBoundary>
    </HashRouter>
  );
}
