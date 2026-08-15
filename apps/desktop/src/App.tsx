import { useCallback, useEffect, useMemo, useState } from 'react';
import { HashRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import AppShell from './components/AppShell';
import ErrorBoundary from './components/ErrorBoundary';
import SplashScreen from './components/SplashScreen';
import { api, type AppSettings } from './lib/api';
import { SessionContext } from './lib/session';
import { ToastProvider } from './lib/toast';
import { CycleProvider } from './lib/cycle';
import Dashboard from './pages/Dashboard';
import Signals from './pages/Signals';
import Trades from './pages/Trades';
import Memory from './pages/Memory';
import Settings from './pages/Settings';
import Onboarding from './pages/Onboarding';
import SymbolDetail from './pages/SymbolDetail';

type PulseAuthReport = {
  ok: boolean;
  authMode: string;
  message: string;
};

function AppRoutes() {
  const navigate = useNavigate();
  const [ready, setReady] = useState(false);
  const [splashDone, setSplashDone] = useState(false);
  const [needsOnboarding, setNeedsOnboarding] = useState(true);
  const [bootMessage, setBootMessage] = useState('Igniting Pulsar…');

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      try {
        const s = await api<AppSettings>('settings_get');
        if (cancelled) return;

        if (!s.onboardingComplete) {
          setNeedsOnboarding(true);
          setReady(true);
          return;
        }

        setBootMessage('Verifying NGX Pulse session…');
        const report = await api<PulseAuthReport>('test_pulse_login');
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
      } catch {
        if (cancelled) return;
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
  const finishSplash = useCallback(() => setSplashDone(true), []);

  if (!splashDone) {
    return (
      <SplashScreen
        ready={ready}
        message={bootMessage}
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
          <Route path="/settings" element={<Settings />} />
        </Route>
      </Routes>
    </SessionContext.Provider>
  );
}

export default function App() {
  const browserMode = typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);

  return (
    <HashRouter>
      <ErrorBoundary>
        <ToastProvider>
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
