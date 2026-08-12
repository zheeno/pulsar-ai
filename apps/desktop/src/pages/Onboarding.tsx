import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type AppSettings } from '../lib/api';

export default function OnboardingWizard() {
  const navigate = useNavigate();
  const [step, setStep] = useState(0);
  const [settings, setSettings] = useState<Partial<AppSettings>>({
    pulseBaseUrl: 'https://ngxpulse.ng/api',
    llmProvider: 'openai',
    llmModel: 'gpt-4o-mini',
  });
  const [pulsePassword, setPulsePassword] = useState('');
  const [llmApiKey, setLlmApiKey] = useState('');
  const [status, setStatus] = useState('');

  useEffect(() => {
    api<AppSettings>('settings_get').then((s) => {
      setSettings(s);
      if (s.onboardingComplete) navigate('/');
    }).catch(() => {});
  }, [navigate]);

  async function saveAndFinish() {
    setStatus('Saving...');
    try {
      await api('settings_set', {
        settings: {
          ...settings,
          pulseConfigured: !!settings.pulseEmail,
          llmConfigured: !!llmApiKey,
          onboardingComplete: true,
        },
        pulsePassword: pulsePassword || undefined,
        llmApiKey: llmApiKey || undefined,
      });
      navigate('/');
    } catch (e) {
      setStatus(String(e));
    }
  }

  return (
    <div style={{ minHeight: '100vh', background: '#0b1220', color: '#e2e8f0', padding: 48 }}>
      <h1 style={{ marginTop: 0 }}>Welcome to Pulsar AI</h1>
      <p style={{ color: '#94a3b8' }}>Configure your NGX Pulse and LLM credentials to get started.</p>

      {step === 0 && (
        <div style={{ maxWidth: 480 }}>
          <h2>NGX Pulse</h2>
          <label style={labelStyle}>Supabase URL</label>
          <input style={inputStyle} value={settings.pulseSupabaseUrl || ''} onChange={(e) => setSettings({ ...settings, pulseSupabaseUrl: e.target.value })} />
          <label style={labelStyle}>Supabase Anon Key</label>
          <input style={inputStyle} value={settings.pulseSupabaseAnonKey || ''} onChange={(e) => setSettings({ ...settings, pulseSupabaseAnonKey: e.target.value })} />
          <label style={labelStyle}>Email</label>
          <input style={inputStyle} value={settings.pulseEmail || ''} onChange={(e) => setSettings({ ...settings, pulseEmail: e.target.value })} />
          <label style={labelStyle}>Password</label>
          <input type="password" style={inputStyle} value={pulsePassword} onChange={(e) => setPulsePassword(e.target.value)} />
          <button style={btnStyle} onClick={() => setStep(1)}>Next</button>
        </div>
      )}

      {step === 1 && (
        <div style={{ maxWidth: 480 }}>
          <h2>LLM Provider</h2>
          <label style={labelStyle}>Provider</label>
          <select style={inputStyle} value={settings.llmProvider} onChange={(e) => setSettings({ ...settings, llmProvider: e.target.value })}>
            <option value="openai">OpenAI</option>
            <option value="anthropic">Anthropic</option>
            <option value="openrouter">OpenRouter</option>
          </select>
          <label style={labelStyle}>Model</label>
          <input style={inputStyle} value={settings.llmModel || ''} onChange={(e) => setSettings({ ...settings, llmModel: e.target.value })} />
          <label style={labelStyle}>API Key</label>
          <input type="password" style={inputStyle} value={llmApiKey} onChange={(e) => setLlmApiKey(e.target.value)} />
          <div style={{ display: 'flex', gap: 12, marginTop: 16 }}>
            <button style={btnSecondary} onClick={() => setStep(0)}>Back</button>
            <button style={btnStyle} onClick={saveAndFinish}>Finish Setup</button>
          </div>
        </div>
      )}

      {status && <p style={{ color: '#f87171' }}>{status}</p>}
    </div>
  );
}

const labelStyle: React.CSSProperties = { display: 'block', marginTop: 12, marginBottom: 4, fontSize: 13, color: '#94a3b8' };
const inputStyle: React.CSSProperties = { width: '100%', padding: 10, borderRadius: 6, border: '1px solid #334155', background: '#1e293b', color: '#e2e8f0' };
const btnStyle: React.CSSProperties = { marginTop: 16, background: '#22c55e', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' };
const btnSecondary: React.CSSProperties = { ...btnStyle, background: '#334155' };
