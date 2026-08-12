import { useCallback, useEffect, useMemo, useState } from 'react';
import { HashRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import ErrorBoundary from './components/ErrorBoundary';
import { api, type AppSettings } from './lib/api';
import { SessionContext } from './lib/session';
import Dashboard from './pages/Dashboard';
import Signals from './pages/Signals';
import Trades from './pages/Trades';
import Strategy from './pages/Strategy';
import Backtest from './pages/Backtest';
import Settings from './pages/Settings';
import Onboarding from './pages/Onboarding';

type PulseAuthReport = {
  ok: boolean;
  authMode: string;
  message: string;
};

function AppRoutes() {
  const navigate = useNavigate();
  const [ready, setReady] = useState(false);
  const [needsOnboarding, setNeedsOnboarding] = useState(true);
  const [bootMessage, setBootMessage] = useState('Loading Pulsar AI…');

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

  if (!ready) {
    return (
      <div style={{ padding: 48, color: '#e2e8f0', background: '#0b1220', minHeight: '100vh' }}>
        {bootMessage}
      </div>
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
          path="/"
          element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <Dashboard />}
        />
        <Route path="/signals" element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <Signals />} />
        <Route path="/trades" element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <Trades />} />
        <Route path="/strategy" element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <Strategy />} />
        <Route path="/backtest" element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <Backtest />} />
        <Route path="/settings" element={needsOnboarding ? <Navigate to="/onboarding" replace /> : <Settings />} />
      </Routes>
    </SessionContext.Provider>
  );
}

export default function App() {
  const browserMode = typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);

  return (
    <HashRouter>
      <ErrorBoundary>
        <div style={{ minHeight: '100vh', background: '#0b1220', color: '#e2e8f0' }}>
          {browserMode && (
            <div style={{
              background: '#422006', color: '#fde68a', padding: '8px 16px', fontSize: 13, textAlign: 'center',
            }}>
              Browser mock mode — sample data in localStorage. Pulse gating requires the desktop app.
            </div>
          )}
          <AppRoutes />
        </div>
      </ErrorBoundary>
    </HashRouter>
  );
}
