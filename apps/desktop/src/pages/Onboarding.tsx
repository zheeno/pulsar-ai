import { useEffect, useId, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { IconSpinner } from '../components/Icons';
import pulsarLogoFull from '../assets/pulsar-logo-full.svg';
import { api, type AppSettings } from '../lib/api';
import { useSession } from '../lib/session';
import { useToast } from '../lib/toast';

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
  const toast = useToast();
  const { setActiveModule } = useSession();
  const emailId = useId();
  const passwordId = useId();
  const providerId = useId();
  const modelId = useId();
  const keyId = useId();
  const [step, setStep] = useState(0);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [chosenModule, setChosenModule] = useState<'stocks' | 'crypto'>('stocks');
  const [pulsePassword, setPulsePassword] = useState('');
  const [llmApiKey, setLlmApiKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [pulseOk, setPulseOk] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    api<AppSettings>('settings_get')
      .then((s) => {
        setSettings(s);
        setChosenModule(s.activeModule === 'crypto' ? 'crypto' : 'stocks');
        if (s.onboardingComplete) {
          onComplete?.();
          navigate('/', { replace: true });
        }
      })
      .catch((e) => {
        setLoadError(String(e));
        toast.error(String(e), 'Setup');
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- mount once
  }, [navigate, onComplete]);

  async function chooseModule() {
    if (!settings) return;
    setBusy(true);
    try {
      await api('settings_set', {
        settings: {
          ...settings,
          activeModule: chosenModule,
          pulseConfigured: false,
          llmConfigured: false,
          onboardingComplete: false,
        },
      });
      setSettings({ ...settings, activeModule: chosenModule });
      setActiveModule(chosenModule);
      setStep(1);
    } catch (e) {
      toast.error(String(e), 'Module');
    } finally {
      setBusy(false);
    }
  }

  async function signInPulse() {
    if (!settings) return;
    const email = (settings.pulseEmail || '').trim();
    if (!email || !pulsePassword) {
      toast.warning('Enter NGX Pulse email and password.', 'Pulse');
      return;
    }

    setBusy(true);
    toast.info('Signing in to NGX Pulse…', 'Pulse');
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
        let msg = report.message
          || (report.authMode === 'mock'
            ? 'Pulse auth is not configured (mock mode). Check .env Supabase URL/anon key.'
            : 'Pulse login failed.');
        if (report.logs?.length) msg = `${msg}\n${report.logs.join('\n')}`;
        toast.error(msg, 'Pulse login failed');
        return;
      }

      setPulseOk(true);
      toast.success('Pulse session verified. Next: add and test your LLM key.', 'Pulse');
      setStep(2);
    } catch (e) {
      setPulseOk(false);
      toast.error(String(e), 'Pulse login failed');
    } finally {
      setBusy(false);
    }
  }

  async function finishWithLlm() {
    if (!settings || !pulseOk) {
      toast.warning('Sign in with NGX Pulse first.', 'Setup');
      setStep(1);
      return;
    }
    if (!llmApiKey.trim()) {
      toast.warning('Enter an LLM API key.', 'LLM');
      return;
    }

    setBusy(true);
    toast.info('Saving LLM key and verifying…', 'LLM');
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

      toast.info('Verifying Pulse + LLM and completing setup…', 'Setup');
      await api('complete_onboarding');

      toast.success('Setup complete. Welcome to Pulsar AI.', 'Ready');
      onComplete?.();
      navigate('/', { replace: true });
    } catch (e) {
      toast.error(String(e), 'Setup failed');
    } finally {
      setBusy(false);
    }
  }

  if (!settings) {
    return (
      <div className="onboarding-screen">
        <div className="onboarding-card">
          <div className="onboarding-brand">
            <img src={pulsarLogoFull} alt="Pulsar AI" className="onboarding-brand__logo" />
          </div>
          <p className="muted" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <IconSpinner /> Loading…
          </p>
          {loadError ? (
            <p className="muted" style={{ marginTop: 16 }}>{loadError}</p>
          ) : null}
        </div>
      </div>
    );
  }

  return (
    <div className="onboarding-screen">
      <div className="onboarding-card">
        <div className="onboarding-brand">
          <img src={pulsarLogoFull} alt="Pulsar AI" className="onboarding-brand__logo" />
        </div>
        <h1 style={{ margin: '0 0 8px', fontSize: '1.5rem', letterSpacing: '-0.03em' }}>
          Protect your trading workspace
        </h1>
        <p className="muted" style={{ marginTop: 0, lineHeight: 1.45 }}>
          Choose Stocks or Crypto, then sign in with NGX Pulse and verify an LLM key. Access unlocks only after Pulse and LLM succeed. The module only changes which market the sandbox trades.
        </p>

        <div className="steps" aria-label="Setup steps">
          <div className={`step-pill${step === 0 ? ' is-active' : ''}${step > 0 ? ' is-done' : ''}`}>1 · Market</div>
          <div className={`step-pill${step === 1 ? ' is-active' : ''}${pulseOk ? ' is-done' : ''}`}>2 · Pulse</div>
          <div className={`step-pill${step === 2 ? ' is-active' : ''}`}>3 · LLM</div>
        </div>

        {step === 0 && (
          <div>
            <h2 style={{ margin: '0 0 8px', fontSize: '1.1rem' }}>Trading workspace</h2>
            <p className="muted" style={{ fontSize: 13, marginTop: 0, lineHeight: 1.45 }}>
              NGX Pulse is required for both. Crypto runs as a 24/7 sandbox with public spot quotes — no live crypto broker in this version.
            </p>
            <div className="broker-grid" role="radiogroup" aria-label="Market module" style={{ marginBottom: 16 }}>
              <button
                type="button"
                role="radio"
                aria-checked={chosenModule === 'stocks'}
                className={`broker-card${chosenModule === 'stocks' ? ' is-selected' : ''}`}
                disabled={busy}
                onClick={() => setChosenModule('stocks')}
              >
                <span className="broker-card__name">Stocks</span>
                <span className="broker-card__meta">NGX sandbox (NGN)</span>
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={chosenModule === 'crypto'}
                className={`broker-card${chosenModule === 'crypto' ? ' is-selected' : ''}`}
                disabled={busy}
                onClick={() => setChosenModule('crypto')}
              >
                <span className="broker-card__name">Crypto</span>
                <span className="broker-card__meta">USDT sandbox, 24/7</span>
              </button>
            </div>
            <div className="btn-row">
              <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void chooseModule()}>
                {busy && <IconSpinner />}
                Continue
              </button>
            </div>
          </div>
        )}

        {step === 1 && (
          <div>
            <h2 style={{ margin: '0 0 8px', fontSize: '1.1rem' }}>NGX Pulse account</h2>
            <p className="muted" style={{ fontSize: 13, marginTop: 0, lineHeight: 1.45 }}>
              Connection details come from the environment. Enter your Pulse email and password.
            </p>
            <label className="label" htmlFor={emailId}>Email</label>
            <input
              id={emailId}
              className="input"
              autoComplete="username"
              disabled={busy}
              value={settings.pulseEmail || ''}
              onChange={(e) => setSettings({ ...settings, pulseEmail: e.target.value })}
            />
            <label className="label" htmlFor={passwordId}>Password</label>
            <input
              id={passwordId}
              className="input"
              type="password"
              autoComplete="current-password"
              disabled={busy}
              value={pulsePassword}
              onChange={(e) => setPulsePassword(e.target.value)}
            />
            <div className="btn-row">
              <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => setStep(0)}>Back</button>
              <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void signInPulse()}>
                {busy && <IconSpinner />}
                {busy ? 'Signing in…' : 'Sign in'}
              </button>
            </div>
          </div>
        )}

        {step === 2 && (
          <div>
            <h2 style={{ margin: '0 0 8px', fontSize: '1.1rem' }}>LLM provider</h2>
            <p className="muted" style={{ fontSize: 13, marginTop: 0, lineHeight: 1.45 }}>
              Pulse is verified. Your API key is tested before the app unlocks.
            </p>
            <label className="label" htmlFor={providerId}>Provider</label>
            <select
              id={providerId}
              className="input"
              disabled={busy}
              value={settings.llmProvider}
              onChange={(e) => setSettings({ ...settings, llmProvider: e.target.value })}
            >
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
              <option value="openrouter">OpenRouter</option>
            </select>
            <label className="label" htmlFor={modelId}>Model</label>
            <input
              id={modelId}
              className="input"
              disabled={busy}
              value={settings.llmModel || ''}
              onChange={(e) => setSettings({ ...settings, llmModel: e.target.value })}
            />
            <label className="label" htmlFor={keyId}>API key</label>
            <input
              id={keyId}
              className="input"
              type="password"
              autoComplete="off"
              disabled={busy}
              value={llmApiKey}
              onChange={(e) => setLlmApiKey(e.target.value)}
            />
            <div className="btn-row">
              <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => setStep(1)}>Back</button>
              <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void finishWithLlm()}>
                {busy && <IconSpinner />}
                {busy ? 'Verifying…' : 'Verify & enter app'}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
