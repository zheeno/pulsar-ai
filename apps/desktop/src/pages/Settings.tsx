import { useEffect, useState } from 'react';
import Nav from '../components/Nav';
import { api, type AppSettings } from '../lib/api';
import { useSession } from '../lib/session';

type PulseAuthReport = {
  ok: boolean;
  authMode: string;
  supabaseHost?: string | null;
  pulseBaseUrl: string;
  hasEmail: boolean;
  email?: string | null;
  hasPassword: boolean;
  hasAnonKey: boolean;
  loginUrl?: string | null;
  httpStatus?: number | null;
  tokenExpiresAt?: number | null;
  tokenPreview?: string | null;
  message: string;
  logs: string[];
};

export default function SettingsPage() {
  const { logout } = useSession();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [pulsePassword, setPulsePassword] = useState('');
  const [llmApiKey, setLlmApiKey] = useState('');
  const [status, setStatus] = useState('');
  const [pulseReport, setPulseReport] = useState<PulseAuthReport | null>(null);

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
      setStatus('Saved — run Test Pulse Login to verify session auth');
      if (pulsePassword) setPulsePassword('');
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function testPulse() {
    setStatus('Testing Pulse… (check the desktop:dev terminal for ngx_pulse logs)');
    setPulseReport(null);
    try {
      const report = await api<PulseAuthReport>('test_pulse_login');
      setPulseReport(report);
      setStatus(report.ok ? `Pulse OK (${report.authMode})` : `Pulse failed (${report.authMode})`);
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
      <div style={{ padding: 24, maxWidth: 720, margin: '0 auto' }}>
        <h1>Settings</h1>

        <section style={sectionStyle}>
          <h2>NGX Pulse</h2>
          <p style={{ color: '#64748b', fontSize: 13, marginTop: 0 }}>
            Supabase URL/anon key come from <code>.env</code>. Email/password are stored locally.
            Pulse HTTP runs in Rust — use <strong>Test Pulse Login</strong> and the terminal logs (not browser Network).
          </p>
          <Field label="Email" value={settings.pulseEmail || ''} onChange={(v) => setSettings({ ...settings, pulseEmail: v })} />
          <Field label="Password (leave blank to keep)" value={pulsePassword} onChange={setPulsePassword} type="password" />
          <button type="button" onClick={() => void testPulse()} style={btnSecondary}>Test Pulse Login</button>
        </section>

        {pulseReport && (
          <section style={sectionStyle}>
            <h2 style={{ color: pulseReport.ok ? '#22c55e' : '#f87171' }}>
              Auth report — {pulseReport.ok ? 'success' : 'failed'}
            </h2>
            <dl style={dlStyle}>
              <dt>authMode</dt><dd>{pulseReport.authMode}</dd>
              <dt>supabaseHost</dt><dd>{pulseReport.supabaseHost || '—'}</dd>
              <dt>loginUrl</dt><dd style={{ wordBreak: 'break-all' }}>{pulseReport.loginUrl || '—'}</dd>
              <dt>pulseBaseUrl</dt><dd>{pulseReport.pulseBaseUrl}</dd>
              <dt>email</dt><dd>{pulseReport.email || '—'}</dd>
              <dt>hasPassword</dt><dd>{String(pulseReport.hasPassword)}</dd>
              <dt>hasAnonKey</dt><dd>{String(pulseReport.hasAnonKey)}</dd>
              <dt>httpStatus</dt><dd>{pulseReport.httpStatus ?? '—'}</dd>
              <dt>tokenPreview</dt><dd>{pulseReport.tokenPreview || '—'}</dd>
              <dt>tokenExpiresAt</dt><dd>{pulseReport.tokenExpiresAt ?? '—'}</dd>
              <dt>message</dt><dd>{pulseReport.message}</dd>
            </dl>
            <h3 style={{ fontSize: 14, color: '#94a3b8' }}>Request log</h3>
            <pre style={preStyle}>{pulseReport.logs.join('\n')}</pre>
          </section>
        )}

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
          <button type="button" onClick={() => void testLlm()} style={btnSecondary}>Test LLM</button>
        </section>

        <div style={{ display: 'flex', gap: 12, marginTop: 24, alignItems: 'center' }}>
          <button type="button" onClick={() => void save()} style={btnPrimary}>Save Settings</button>
          <button
            type="button"
            onClick={() => {
              void (async () => {
                setStatus('Logging out…');
                try {
                  await logout();
                } catch (e) {
                  setStatus(String(e));
                }
              })();
            }}
            style={{ ...btnPrimary, background: '#7f1d1d' }}
          >
            Log out
          </button>
        </div>
        {status && (
          <p style={{
            marginTop: 12,
            color: status.includes('OK') || status.includes('success') ? '#22c55e' : '#94a3b8',
            whiteSpace: 'pre-wrap',
          }}
          >
            {status}
          </p>
        )}
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
const dlStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: '160px 1fr',
  gap: '8px 16px',
  fontSize: 13,
  margin: 0,
};
const preStyle: React.CSSProperties = {
  background: '#0f172a',
  padding: 12,
  borderRadius: 6,
  fontSize: 12,
  overflow: 'auto',
  color: '#cbd5e1',
  whiteSpace: 'pre-wrap',
};
