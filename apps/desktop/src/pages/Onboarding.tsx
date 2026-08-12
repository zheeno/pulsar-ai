import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type AppSettings } from '../lib/api';

type Props = {
  onComplete?: () => void;
};

type PulseAuthReport = {
  ok: boolean;
  authMode: string;
  message: string;
  logs?: string[];
};

export default function OnboardingWizard({ onComplete }: Props) {
  const navigate = useNavigate();
  const [step, setStep] = useState(0);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [pulsePassword, setPulsePassword] = useState('');
  const [llmApiKey, setLlmApiKey] = useState('');
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState(false);
  const [pulseOk, setPulseOk] = useState(false);

  useEffect(() => {
    api<AppSettings>('settings_get')
      .then((s) => {
        setSettings(s);
        if (s.onboardingComplete) {
          onComplete?.();
          navigate('/', { replace: true });
        }
      })
      .catch((e) => setStatus(String(e)));
  }, [navigate, onComplete]);

  async function signInPulse() {
    if (!settings) return;
    const email = (settings.pulseEmail || '').trim();
    if (!email || !pulsePassword) {
      setStatus('Enter NGX Pulse email and password.');
      return;
    }

    setBusy(true);
    setStatus('Signing in to NGX Pulse…');
    try {
      await api('settings_set', {
        settings: {
          ...settings,
          pulseEmail: email,
          pulseConfigured: false,
          llmConfigured: false,
          onboardingComplete: false,
        },
        pulsePassword,
      });

      const report = await api<PulseAuthReport>('test_pulse_login');
      if (!report.ok || report.authMode !== 'session') {
        setPulseOk(false);
        setStatus(
          report.message
            || (report.authMode === 'mock'
              ? 'Pulse auth is not configured (mock mode). Check .env Supabase URL/anon key.'
              : 'Pulse login failed.'),
        );
        if (report.logs?.length) {
          setStatus((prev) => `${prev}\n${report.logs!.join('\n')}`);
        }
        return;
      }

      setPulseOk(true);
      setStatus('Pulse session OK. Configure your LLM key.');
      setStep(1);
    } catch (e) {
      setPulseOk(false);
      setStatus(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finishWithLlm() {
    if (!settings || !pulseOk) {
      setStatus('Sign in with NGX Pulse first.');
      setStep(0);
      return;
    }
    if (!llmApiKey.trim()) {
      setStatus('Enter an LLM API key.');
      return;
    }

    setBusy(true);
    setStatus('Saving LLM key and verifying…');
    try {
      await api('settings_set', {
        settings: {
          ...settings,
          pulseConfigured: false,
          llmConfigured: false,
          onboardingComplete: false,
        },
        llmApiKey: llmApiKey.trim(),
      });

      await api('test_llm');

      setStatus('Verifying Pulse + LLM and completing setup…');
      await api('complete_onboarding');

      onComplete?.();
      navigate('/', { replace: true });
    } catch (e) {
      setStatus(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (!settings) {
    return (
      <div style={{ minHeight: '100vh', background: '#0b1220', color: '#e2e8f0', padding: 48 }}>
        Loading settings...
        {status && <p style={{ color: '#f87171' }}>{status}</p>}
      </div>
    );
  }

  return (
    <div style={{ minHeight: '100vh', background: '#0b1220', color: '#e2e8f0', padding: 48 }}>
      <h1 style={{ marginTop: 0 }}>Welcome to Pulsar AI</h1>
      <p style={{ color: '#94a3b8' }}>
        Sign in with NGX Pulse, then verify an LLM API key to unlock the app.
      </p>

      {step === 0 && (
        <div style={{ maxWidth: 480 }}>
          <h2>1. NGX Pulse</h2>
          <p style={{ color: '#64748b', fontSize: 13 }}>
            Supabase endpoint comes from the environment. Invalid credentials cannot continue.
          </p>
          <label style={labelStyle}>Email</label>
          <input
            style={inputStyle}
            disabled={busy}
            value={settings.pulseEmail || ''}
            onChange={(e) => setSettings({ ...settings, pulseEmail: e.target.value })}
          />
          <label style={labelStyle}>Password</label>
          <input
            type="password"
            style={inputStyle}
            disabled={busy}
            value={pulsePassword}
            onChange={(e) => setPulsePassword(e.target.value)}
          />
          <button style={btnStyle} disabled={busy} onClick={() => void signInPulse()}>
            {busy ? 'Signing in…' : 'Sign in'}
          </button>
        </div>
      )}

      {step === 1 && (
        <div style={{ maxWidth: 480 }}>
          <h2>2. LLM Provider</h2>
          <p style={{ color: '#64748b', fontSize: 13 }}>
            Pulse session verified. The API key is tested before access is granted.
          </p>
          <label style={labelStyle}>Provider</label>
          <select
            style={inputStyle}
            disabled={busy}
            value={settings.llmProvider}
            onChange={(e) => setSettings({ ...settings, llmProvider: e.target.value })}
          >
            <option value="openai">OpenAI</option>
            <option value="anthropic">Anthropic</option>
            <option value="openrouter">OpenRouter</option>
          </select>
          <label style={labelStyle}>Model</label>
          <input
            style={inputStyle}
            disabled={busy}
            value={settings.llmModel || ''}
            onChange={(e) => setSettings({ ...settings, llmModel: e.target.value })}
          />
          <label style={labelStyle}>API Key</label>
          <input
            type="password"
            style={inputStyle}
            disabled={busy}
            value={llmApiKey}
            onChange={(e) => setLlmApiKey(e.target.value)}
          />
          <div style={{ display: 'flex', gap: 12, marginTop: 16 }}>
            <button style={btnSecondary} disabled={busy} onClick={() => setStep(0)}>Back</button>
            <button style={btnStyle} disabled={busy} onClick={() => void finishWithLlm()}>
              {busy ? 'Verifying…' : 'Verify & enter app'}
            </button>
          </div>
        </div>
      )}

      {status && (
        <pre style={{
          marginTop: 24,
          maxWidth: 640,
          color: status.includes('OK') || status.includes('verified') ? '#86efac' : '#f87171',
          whiteSpace: 'pre-wrap',
          fontSize: 13,
        }}
        >
          {status}
        </pre>
      )}
    </div>
  );
}

const labelStyle: React.CSSProperties = { display: 'block', marginTop: 12, marginBottom: 4, fontSize: 13, color: '#94a3b8' };
const inputStyle: React.CSSProperties = {
  width: '100%', padding: 10, borderRadius: 6, border: '1px solid #334155', background: '#1e293b', color: '#e2e8f0',
};
const btnStyle: React.CSSProperties = {
  marginTop: 16, background: '#22c55e', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer',
};
const btnSecondary: React.CSSProperties = { ...btnStyle, background: '#334155' };
