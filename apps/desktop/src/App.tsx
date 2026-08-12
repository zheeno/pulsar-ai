import { useEffect, useState } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
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
  const [onboarding, setOnboarding] = useState(true);

  useEffect(() => {
    api<AppSettings>('settings_get')
      .then((s) => {
        setOnboarding(!s.onboardingComplete);
        setReady(true);
      })
      .catch(() => setReady(true));
  }, []);

  if (!ready) {
    return <div style={{ padding: 48, color: '#e2e8f0', background: '#0b1220', minHeight: '100vh' }}>Loading Pulsar AI...</div>;
  }

  return (
    <Routes>
      <Route path="/onboarding" element={<Onboarding />} />
      <Route path="/" element={onboarding ? <Navigate to="/onboarding" /> : <Dashboard />} />
      <Route path="/signals" element={<Signals />} />
      <Route path="/trades" element={<Trades />} />
      <Route path="/strategy" element={<Strategy />} />
      <Route path="/backtest" element={<Backtest />} />
      <Route path="/settings" element={<Settings />} />
    </Routes>
  );
}

export default function App() {
  const browserMode = typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);

  return (
    <BrowserRouter>
      <div style={{ minHeight: '100vh', background: '#0b1220', color: '#e2e8f0' }}>
        {browserMode && (
          <div style={{
            background: '#422006', color: '#fde68a', padding: '8px 16px', fontSize: 13, textAlign: 'center',
          }}>
            Browser mock mode — sample data in localStorage. Install Xcode CLT + run `npm run desktop:dev` for the full local app.
          </div>
        )}
        <AppRoutes />
      </div>
    </BrowserRouter>
  );
}
