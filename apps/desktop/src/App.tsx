import { useCallback, useEffect, useState } from 'react';
import { HashRouter, Navigate, Route, Routes } from 'react-router-dom';
import ErrorBoundary from './components/ErrorBoundary';
import { api, type AppSettings } from './lib/api';
import Dashboard from './pages/Dashboard';
import Signals from './pages/Signals';
import Trades from './pages/Trades';
import Strategy from './pages/Strategy';
import Backtest from './pages/Backtest';
import Settings from './pages/Settings';
import Onboarding from './pages/Onboarding';

function AppRoutes() {
  const [ready, setReady] = useState(false);
  const [needsOnboarding, setNeedsOnboarding] = useState(true);

  useEffect(() => {
    api<AppSettings>('settings_get')
      .then((s) => setNeedsOnboarding(!s.onboardingComplete))
      .catch(() => setNeedsOnboarding(true))
      .finally(() => setReady(true));
  }, []);

  const markOnboardingComplete = useCallback(() => {
    setNeedsOnboarding(false);
  }, []);

  if (!ready) {
    return (
      <div style={{ padding: 48, color: '#e2e8f0', background: '#0b1220', minHeight: '100vh' }}>
        Loading Pulsar AI...
      </div>
    );
  }

  return (
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
              Browser mock mode — sample data in localStorage.
            </div>
          )}
          <AppRoutes />
        </div>
      </ErrorBoundary>
    </HashRouter>
  );
}
