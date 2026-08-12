import { useEffect, useState } from 'react';
import Nav from '../components/Nav';
import { api, type AppSettings } from '../lib/api';

export default function SettingsPage() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [pulsePassword, setPulsePassword] = useState('');
  const [llmApiKey, setLlmApiKey] = useState('');
  const [status, setStatus] = useState('');

  useEffect(() => {
    api<AppSettings>('settings_get').then(setSettings).catch(() => {});
  }, []);

  async function save() {
    if (!settings) return;
    setStatus('Saving...');
    try {
      await api('settings_set', {
        settings: {
          ...settings,
          pulseConfigured: !!settings.pulseEmail,
          llmConfigured: settings.llmConfigured || !!llmApiKey,
        },
        pulsePassword: pulsePassword || undefined,
        llmApiKey: llmApiKey || undefined,
      });
      setStatus('Saved');
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function testPulse() {
    setStatus('Testing Pulse...');
    try {
      await api('test_pulse_login');
      setStatus('Pulse login OK');
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function testLlm() {
    setStatus('Testing LLM...');
    try {
      const r = await api<{ message: string }>('test_llm');
      setStatus(`LLM OK: ${r.message?.slice(0, 50)}`);
    } catch (e) {
      setStatus(String(e));
    }
  }

  if (!settings) return <div style={{ padding: 24, color: '#e2e8f0' }}>Loading...</div>;

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 640, margin: '0 auto' }}>
        <h1>Settings</h1>

        <section style={sectionStyle}>
          <h2>NGX Pulse</h2>
          <Field label="Supabase URL" value={settings.pulseSupabaseUrl || ''} onChange={(v) => setSettings({ ...settings, pulseSupabaseUrl: v })} />
          <Field label="Anon Key" value={settings.pulseSupabaseAnonKey || ''} onChange={(v) => setSettings({ ...settings, pulseSupabaseAnonKey: v })} />
          <Field label="Email" value={settings.pulseEmail || ''} onChange={(v) => setSettings({ ...settings, pulseEmail: v })} />
          <Field label="Password (leave blank to keep)" value={pulsePassword} onChange={setPulsePassword} type="password" />
          <button onClick={testPulse} style={btnSecondary}>Test Pulse Login</button>
        </section>

        <section style={sectionStyle}>
          <h2>LLM</h2>
          <label style={labelStyle}>Provider</label>
          <select style={inputStyle} value={settings.llmProvider} onChange={(e) => setSettings({ ...settings, llmProvider: e.target.value })}>
            <option value="openai">OpenAI</option>
            <option value="anthropic">Anthropic</option>
            <option value="openrouter">OpenRouter</option>
          </select>
          <Field label="Model" value={settings.llmModel} onChange={(v) => setSettings({ ...settings, llmModel: v })} />
          <Field label="Base URL (optional)" value={settings.llmBaseUrl || ''} onChange={(v) => setSettings({ ...settings, llmBaseUrl: v })} />
          <Field label="API Key (leave blank to keep)" value={llmApiKey} onChange={setLlmApiKey} type="password" />
          <button onClick={testLlm} style={btnSecondary}>Test LLM</button>
        </section>

        <div style={{ display: 'flex', gap: 12, marginTop: 24 }}>
          <button onClick={save} style={btnPrimary}>Save Settings</button>
        </div>
        {status && <p style={{ marginTop: 12, color: status.includes('OK') ? '#22c55e' : '#94a3b8' }}>{status}</p>}
      </div>
    </div>
  );
}

function Field({ label, value, onChange, type = 'text' }: { label: string; value: string; onChange: (v: string) => void; type?: string }) {
  return (
    <>
      <label style={labelStyle}>{label}</label>
      <input type={type} style={inputStyle} value={value} onChange={(e) => onChange(e.target.value)} />
    </>
  );
}

const sectionStyle: React.CSSProperties = { background: '#1e293b', padding: 24, borderRadius: 8, marginTop: 16 };
const labelStyle: React.CSSProperties = { display: 'block', marginTop: 12, marginBottom: 4, fontSize: 13, color: '#94a3b8' };
const inputStyle: React.CSSProperties = { width: '100%', padding: 10, borderRadius: 6, border: '1px solid #334155', background: '#0f172a', color: '#e2e8f0' };
const btnPrimary: React.CSSProperties = { background: '#22c55e', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' };
const btnSecondary: React.CSSProperties = { ...btnPrimary, background: '#334155', marginTop: 12 };
