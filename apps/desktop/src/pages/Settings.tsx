import { useEffect, useId, useRef, useState } from 'react';
import { IconLogout, IconSpinner } from '../components/Icons';
import { api, type AppSettings } from '../lib/api';
import { useSession } from '../lib/session';
import { useToast } from '../lib/toast';

type LlmStatus = {
  provider: string;
  model: string;
  baseUrl?: string | null;
  configured: boolean;
  maskedKey?: string | null;
};

type PulseProfile = {
  ok: boolean;
  authMode: string;
  email?: string | null;
  userId?: string | null;
  displayName?: string | null;
  phone?: string | null;
  createdAt?: string | null;
  lastSignInAt?: string | null;
  emailConfirmed?: boolean | null;
  message: string;
};

type WealthProfile = {
  ok: boolean;
  connected: boolean;
  email?: string | null;
  tradingProfile?: string | null;
  tradingVerified: boolean;
  tradingMode: string;
  brokerageBalance?: number | null;
  availableBalance?: number | null;
  currentBalance?: number | null;
  baseUrl: string;
  message: string;
  displayName?: string | null;
};

type WealthLoginResult = {
  ok: boolean;
  needs2fa: boolean;
  tempToken?: string | null;
  connected: boolean;
  message: string;
  email?: string | null;
  baseUrl: string;
};

type StrategyRecord = {
  id: string;
  name: string;
  max_position_pct: number;
  max_daily_trades: number;
  stop_loss_pct: number;
  take_profit_pct?: number | null;
  min_confidence_to_trade: number;
  max_daily_drawdown_pct: number;
  position_size_pct: number;
};

type StrategyDraft = {
  maxPositionPct: number;
  positionSizePct: number;
  maxDailyTrades: number;
  minConfidenceToTrade: number;
  maxDailyDrawdownPct: number;
  stopLossPct: number;
  takeProfitPct: number;
};

const PROVIDER_LABELS: Record<string, string> = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  openrouter: 'OpenRouter',
};

const AUTH_LABELS: Record<string, string> = {
  session: 'NGX Pulse session',
  api_key: 'API key',
  mock: 'Demo mode',
};

function clamp(n: number, min: number, max: number) {
  return Math.min(max, Math.max(min, n));
}

function toPct(ratio: number, min: number, max: number) {
  return clamp(Math.round(ratio * 100), min, max);
}

function draftFromStrategy(s: StrategyRecord): StrategyDraft {
  return {
    maxPositionPct: toPct(s.max_position_pct, 1, 50),
    positionSizePct: toPct(s.position_size_pct, 1, 25),
    maxDailyTrades: clamp(Math.round(s.max_daily_trades), 1, 20),
    minConfidenceToTrade: toPct(s.min_confidence_to_trade, 40, 95),
    maxDailyDrawdownPct: toPct(s.max_daily_drawdown_pct, 1, 15),
    stopLossPct: toPct(s.stop_loss_pct, 1, 25),
    takeProfitPct: toPct(s.take_profit_pct ?? 0.1, 2, 40),
  };
}

function initialsFrom(name?: string | null, email?: string | null): string {
  const source = (name || email || 'NG').trim();
  const parts = source.includes('@')
    ? source.split('@')[0].split(/[.\s_-]+/)
    : source.split(/\s+/);
  const letters = parts.filter(Boolean).slice(0, 2).map((p) => p[0]?.toUpperCase() || '');
  return (letters.join('') || 'NG').slice(0, 2);
}

function formatDate(iso?: string | null): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

function formatDateTime(iso?: string | null): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function shortId(id?: string | null): string {
  if (!id) return '—';
  if (id.length <= 12) return id;
  return `${id.slice(0, 8)}…${id.slice(-4)}`;
}

function ParamSlider({
  id,
  label,
  hint,
  value,
  min,
  max,
  step = 1,
  format,
  disabled,
  onChange,
}: {
  id: string;
  label: string;
  hint: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  format: (v: number) => string;
  disabled?: boolean;
  onChange: (v: number) => void;
}) {
  return (
    <div className="param-slider">
      <div className="param-slider__head">
        <div className="param-slider__copy">
          <label className="param-slider__label" htmlFor={id}>{label}</label>
          <p className="param-slider__hint">{hint}</p>
        </div>
        <span className="param-slider__value mono">{format(value)}</span>
      </div>
      <input
        id={id}
        className="param-slider__input"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <div className="param-slider__bounds">
        <span>{format(min)}</span>
        <span>{format(max)}</span>
      </div>
    </div>
  );
}

export default function SettingsPage() {
  const { logout } = useSession();
  const toast = useToast();
  const providerId = useId();
  const modelId = useId();
  const baseId = useId();
  const keyId = useId();
  const titleId = useId();
  const maxPosId = useId();
  const posSizeId = useId();
  const maxTradesId = useId();
  const minConfId = useId();
  const maxDdId = useId();
  const stopLossId = useId();
  const takeProfitId = useId();
  const autoCycleId = useId();
  const cycleIntervalId = useId();
  const firstFieldRef = useRef<HTMLSelectElement>(null);

  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [llmStatus, setLlmStatus] = useState<LlmStatus | null>(null);
  const [profile, setProfile] = useState<PulseProfile | null>(null);
  const [wealth, setWealth] = useState<WealthProfile | null>(null);
  const [wealthEmail, setWealthEmail] = useState('');
  const [wealthPassword, setWealthPassword] = useState('');
  const [wealth2fa, setWealth2fa] = useState('');
  const [wealthTempToken, setWealthTempToken] = useState<string | null>(null);
  const [wealthBusy, setWealthBusy] = useState(false);
  const [strategyDraft, setStrategyDraft] = useState<StrategyDraft | null>(null);
  const [autoCycleEnabled, setAutoCycleEnabled] = useState(false);
  const [autoCycleMinutes, setAutoCycleMinutes] = useState(30);
  const [busy, setBusy] = useState(false);
  const [strategyBusy, setStrategyBusy] = useState(false);
  const [cycleBusy, setCycleBusy] = useState(false);

  const [llmModalOpen, setLlmModalOpen] = useState(false);
  const [draftProvider, setDraftProvider] = useState('openai');
  const [draftModel, setDraftModel] = useState('');
  const [draftBaseUrl, setDraftBaseUrl] = useState('');
  const [draftApiKey, setDraftApiKey] = useState('');
  const [modalStatus, setModalStatus] = useState('');
  const [modalBusy, setModalBusy] = useState(false);
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [resetConfirm, setResetConfirm] = useState('');
  const [resetBusy, setResetBusy] = useState(false);

  async function refresh() {
    const [s, llm, strategy] = await Promise.all([
      api<AppSettings>('settings_get'),
      api<LlmStatus>('llm_status'),
      api<StrategyRecord>('get_strategy'),
    ]);
    setSettings(s);
    setLlmStatus(llm);
    setStrategyDraft(draftFromStrategy(strategy));
    setAutoCycleEnabled(!!s.autoCycleEnabled);
    setAutoCycleMinutes(clamp(Math.round(s.autoCycleIntervalMinutes || 30), 5, 120));
    try {
      setDataDir(await api<string>('app_data_dir'));
    } catch {
      setDataDir(null);
    }
    try {
      setProfile(await api<PulseProfile>('pulse_profile'));
    } catch (e) {
      setProfile({
        ok: false,
        authMode: 'mock',
        email: s.pulseEmail,
        message: String(e),
      });
    }
    try {
      const w = await api<WealthProfile>('wealth_profile');
      setWealth(w);
      if (w.email) setWealthEmail(w.email);
    } catch {
      setWealth({
        ok: false,
        connected: false,
        tradingVerified: false,
        tradingMode: 'sandbox',
        baseUrl: '',
        message: 'Wealth status unavailable.',
      });
    }
  }

  useEffect(() => {
    refresh().catch(() => {});
  }, []);

  useEffect(() => {
    if (!llmModalOpen) return;
    firstFieldRef.current?.focus();
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && !modalBusy) closeLlmModal();
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [llmModalOpen, modalBusy]);

  function openLlmModal() {
    if (!settings && !llmStatus) return;
    setDraftProvider(llmStatus?.provider || settings?.llmProvider || 'openai');
    setDraftModel(llmStatus?.model || settings?.llmModel || '');
    setDraftBaseUrl(llmStatus?.baseUrl || settings?.llmBaseUrl || '');
    setDraftApiKey('');
    setModalStatus('');
    setLlmModalOpen(true);
  }

  function closeLlmModal() {
    if (modalBusy) return;
    setLlmModalOpen(false);
    setDraftApiKey('');
    setModalStatus('');
  }

  async function saveLlmFromModal() {
    if (!settings) return;
    if (!draftApiKey.trim()) {
      setModalStatus('Enter a new API key to update LLM credentials.');
      return;
    }
    setModalBusy(true);
    setModalStatus('Saving and verifying LLM…');
    try {
      await api('settings_set', {
        settings: {
          ...settings,
          llmProvider: draftProvider,
          llmModel: draftModel,
          llmBaseUrl: draftBaseUrl.trim() || undefined,
          llmConfigured: true,
        },
        llmApiKey: draftApiKey.trim(),
      });
      await api('test_llm');
      await refresh();
      setLlmModalOpen(false);
      setDraftApiKey('');
      toast.success('LLM settings updated and verified.', 'LLM');
    } catch (e) {
      setModalStatus(String(e));
    } finally {
      setModalBusy(false);
    }
  }

  async function saveStrategy() {
    if (!strategyDraft) return;
    setStrategyBusy(true);
    toast.info('Saving strategy…');
    try {
      const updated = await api<StrategyRecord>('update_strategy', {
        strategy: {
          maxPositionPct: strategyDraft.maxPositionPct / 100,
          positionSizePct: strategyDraft.positionSizePct / 100,
          maxDailyTrades: strategyDraft.maxDailyTrades,
          minConfidenceToTrade: strategyDraft.minConfidenceToTrade / 100,
          maxDailyDrawdownPct: strategyDraft.maxDailyDrawdownPct / 100,
          stopLossPct: strategyDraft.stopLossPct / 100,
          takeProfitPct: strategyDraft.takeProfitPct / 100,
        },
      });
      setStrategyDraft(draftFromStrategy(updated));
      toast.success('Strategy parameters saved.', 'Strategy');
    } catch (e) {
      toast.error(String(e), 'Strategy');
    } finally {
      setStrategyBusy(false);
    }
  }

  async function saveAutoCycle() {
    if (!settings) return;
    setCycleBusy(true);
    toast.info('Saving automation…');
    try {
      const minutes = clamp(autoCycleMinutes, 5, 120);
      await api('settings_set', {
        settings: {
          ...settings,
          autoCycleEnabled,
          autoCycleIntervalMinutes: minutes,
        },
      });
      setSettings({
        ...settings,
        autoCycleEnabled,
        autoCycleIntervalMinutes: minutes,
      });
      setAutoCycleMinutes(minutes);
      if (autoCycleEnabled) {
        toast.success(
          `Automatic cycles enabled every ${minutes} minutes during market hours.`,
          'Automation',
        );
      } else {
        toast.info(
          'Automatic cycles disabled. You can still run a cycle manually from Home.',
          'Automation',
        );
      }
    } catch (e) {
      toast.error(String(e), 'Automation');
    } finally {
      setCycleBusy(false);
    }
  }

  if (!settings || !llmStatus || !profile || !strategyDraft) {
    return (
      <div className="page">
        <div className="skeleton skeleton-line" style={{ width: '40%', height: 28 }} />
        <div className="skeleton skeleton-card profile-card" style={{ marginTop: 24, minHeight: 180 }} />
        <div className="skeleton skeleton-card" style={{ marginTop: 20, minHeight: 140 }} />
      </div>
    );
  }

  const providerLabel = PROVIDER_LABELS[llmStatus.provider] || llmStatus.provider;
  const displayName = profile.displayName || profile.email || 'NGX account';
  const authLabel = AUTH_LABELS[profile.authMode] || profile.authMode;
  const sessionLive = profile.ok && profile.authMode === 'session';

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Settings</h1>
          <p>Profile, strategy risk parameters, and LLM credentials.</p>
        </div>
      </header>

      <section className="profile-card" aria-labelledby="profile-heading">
        <div className="profile-card__hero">
          <div className="profile-card__avatar" aria-hidden>
            {initialsFrom(profile.displayName, profile.email)}
          </div>
          <div className="profile-card__identity">
            <div className="profile-card__eyebrow">NGX Pulse</div>
            <h2 id="profile-heading" className="profile-card__name">
              {displayName}
            </h2>
            {profile.email ? (
              <p className="profile-card__email">{profile.email}</p>
            ) : (
              <p className="profile-card__email muted">No email on file</p>
            )}
            <div className="profile-card__badges">
              <span className={`status-pill ${sessionLive ? 'status-pill--ok' : profile.ok ? 'status-pill--warn' : 'status-pill--bad'}`}>
                <span className={`live-dot ${sessionLive ? '' : 'live-dot--off'}`} aria-hidden />
                {sessionLive ? 'Session active' : profile.ok ? authLabel : 'Session unavailable'}
              </span>
              {profile.emailConfirmed === true ? (
                <span className="status-pill status-pill--muted">Email verified</span>
              ) : null}
            </div>
          </div>
        </div>

        <dl className="profile-meta">
          <div className="profile-meta__item">
            <dt>Auth</dt>
            <dd>{authLabel}</dd>
          </div>
          <div className="profile-meta__item">
            <dt>User ID</dt>
            <dd className="mono" title={profile.userId || undefined}>
              {shortId(profile.userId)}
            </dd>
          </div>
          <div className="profile-meta__item">
            <dt>Member since</dt>
            <dd>{formatDate(profile.createdAt)}</dd>
          </div>
          <div className="profile-meta__item">
            <dt>Last sign-in</dt>
            <dd>{formatDateTime(profile.lastSignInAt)}</dd>
          </div>
          {profile.phone ? (
            <div className="profile-meta__item">
              <dt>Phone</dt>
              <dd className="mono">{profile.phone}</dd>
            </div>
          ) : null}
        </dl>

        {!profile.ok && (
          <div className="banner banner-bad" style={{ marginTop: 16 }} role="status">
            {profile.message}
          </div>
        )}

        <div className="profile-card__actions">
          <p className="muted profile-card__hint">
            Pulse credentials are set during onboarding. Log out to reconnect with a different account.
          </p>
          <button
            type="button"
            className="btn btn-danger"
            disabled={busy}
            onClick={() => {
              void (async () => {
                setBusy(true);
                toast.info('Logging out…');
                try {
                  await logout();
                } catch (e) {
                  toast.error(String(e), 'Logout failed');
                  setBusy(false);
                }
              })();
            }}
          >
            {busy ? <IconSpinner /> : <IconLogout />}
            Log out
          </button>
        </div>
      </section>

      <section className="panel" aria-labelledby="wealth-heading" style={{ marginTop: 20 }}>
        <h2 id="wealth-heading">Coronation Wealth</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Without a connected Wealth account, Pulsar runs in sandbox mode. Connect a trading-verified
          account to switch to live trader mode — cash and portfolio come from Wealth, and approved
          signals place real market orders.
        </p>

        {wealth?.connected ? (
          <>
            <div className="profile-card__badges" style={{ marginBottom: 12 }}>
              <span className={`status-pill ${wealth.tradingMode === 'live' ? 'status-pill--ok' : 'status-pill--warn'}`}>
                <span className={`live-dot ${wealth.tradingMode === 'live' ? '' : 'live-dot--off'}`} aria-hidden />
                {wealth.tradingMode === 'live' ? 'Live trader' : 'Sandbox (not verified)'}
              </span>
              {wealth.tradingProfile ? (
                <span className="status-pill status-pill--muted">
                  Trading profile: {wealth.tradingProfile}
                </span>
              ) : null}
            </div>
            <dl className="profile-meta">
              <div className="profile-meta__item">
                <dt>Account</dt>
                <dd>{wealth.displayName || wealth.email || '—'}</dd>
              </div>
              <div className="profile-meta__item">
                <dt>Brokerage balance</dt>
                <dd className="mono">
                  {wealth.brokerageBalance != null
                    ? `₦${Number(wealth.brokerageBalance).toLocaleString('en-NG', { maximumFractionDigits: 2 })}`
                    : '—'}
                </dd>
              </div>
              <div className="profile-meta__item">
                <dt>API</dt>
                <dd className="mono" style={{ fontSize: 12 }}>{wealth.baseUrl || '—'}</dd>
              </div>
            </dl>
            {!wealth.ok && (
              <div className="banner banner-bad" style={{ marginTop: 12 }} role="status">
                {wealth.message}
              </div>
            )}
            {wealth.ok && !wealth.tradingVerified && (
              <div className="banner banner-warn" style={{ marginTop: 12 }} role="status">
                {wealth.message}
              </div>
            )}
            <div style={{ marginTop: 16 }}>
              <button
                type="button"
                className="btn btn-secondary"
                disabled={wealthBusy}
                onClick={() => {
                  void (async () => {
                    setWealthBusy(true);
                    try {
                      await api('wealth_logout');
                      setWealthTempToken(null);
                      setWealthPassword('');
                      setWealth2fa('');
                      toast.success('Wealth account disconnected. Sandbox mode restored.', 'Wealth');
                      await refresh();
                    } catch (e) {
                      toast.error(String(e), 'Wealth logout failed');
                    } finally {
                      setWealthBusy(false);
                    }
                  })();
                }}
              >
                {wealthBusy ? <IconSpinner /> : null}
                Disconnect Wealth
              </button>
            </div>
          </>
        ) : wealthTempToken ? (
          <div className="form-stack" style={{ maxWidth: 420 }}>
            <label>
              <span>2FA code</span>
              <input
                type="text"
                inputMode="numeric"
                autoComplete="one-time-code"
                value={wealth2fa}
                onChange={(e) => setWealth2fa(e.target.value)}
                placeholder="Enter authentication code"
              />
            </label>
            <div style={{ display: 'flex', gap: 8 }}>
              <button
                type="button"
                className="btn btn-primary"
                disabled={wealthBusy || !wealth2fa.trim()}
                onClick={() => {
                  void (async () => {
                    setWealthBusy(true);
                    try {
                      const result = await api<WealthLoginResult>('wealth_verify_2fa', {
                        email: wealthEmail.trim(),
                        tempToken: wealthTempToken,
                        code: wealth2fa.trim(),
                      });
                      if (!result.ok) {
                        toast.error(result.message, 'Wealth 2FA');
                        return;
                      }
                      setWealthTempToken(null);
                      setWealthPassword('');
                      setWealth2fa('');
                      toast.success(result.message, 'Wealth');
                      await refresh();
                    } catch (e) {
                      toast.error(String(e), 'Wealth 2FA');
                    } finally {
                      setWealthBusy(false);
                    }
                  })();
                }}
              >
                {wealthBusy ? <IconSpinner /> : null}
                Verify
              </button>
              <button
                type="button"
                className="btn btn-secondary"
                disabled={wealthBusy}
                onClick={() => {
                  setWealthTempToken(null);
                  setWealth2fa('');
                }}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <div className="form-stack" style={{ maxWidth: 420 }}>
            <label>
              <span>Email</span>
              <input
                type="email"
                autoComplete="username"
                value={wealthEmail}
                onChange={(e) => setWealthEmail(e.target.value)}
                placeholder="you@example.com"
              />
            </label>
            <label>
              <span>Password</span>
              <input
                type="password"
                autoComplete="current-password"
                value={wealthPassword}
                onChange={(e) => setWealthPassword(e.target.value)}
                placeholder="Wealth app password"
              />
            </label>
            <button
              type="button"
              className="btn btn-primary"
              disabled={wealthBusy || !wealthEmail.trim() || !wealthPassword}
              onClick={() => {
                void (async () => {
                  setWealthBusy(true);
                  try {
                    const result = await api<WealthLoginResult>('wealth_login', {
                      email: wealthEmail.trim(),
                      password: wealthPassword,
                    });
                    if (result.needs2fa && result.tempToken) {
                      setWealthTempToken(result.tempToken);
                      toast.info('Enter your 2FA code to finish connecting.', 'Wealth');
                      return;
                    }
                    if (!result.ok) {
                      toast.error(result.message, 'Wealth login');
                      return;
                    }
                    setWealthPassword('');
                    toast.success(result.message, 'Wealth');
                    await refresh();
                  } catch (e) {
                    toast.error(String(e), 'Wealth login');
                  } finally {
                    setWealthBusy(false);
                  }
                })();
              }}
            >
              {wealthBusy ? <IconSpinner /> : null}
              Connect Wealth
            </button>
          </div>
        )}
      </section>

      <section className="panel" aria-labelledby="strategy-heading">
        <h2 id="strategy-heading">Strategy</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          {wealth?.tradingMode === 'live'
            ? 'Risk and sizing for live Wealth orders. The trading universe is all active NGX instruments from Pulse.'
            : 'Risk and sizing for the sandbox. The trading universe is all active NGX instruments from Pulse.'}
        </p>

        <div className="param-slider-grid">
          <ParamSlider
            id={maxPosId}
            label="Max position"
            hint="Caps how large any single holding can grow as a share of total equity. Higher values concentrate risk in fewer names; lower values force broader diversification."
            value={strategyDraft.maxPositionPct}
            min={1}
            max={50}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, maxPositionPct: v })}
          />
          <ParamSlider
            id={posSizeId}
            label="Position size"
            hint="Target size for each new buy as a percent of equity. This sets the default order size before max-position and cash limits are applied."
            value={strategyDraft.positionSizePct}
            min={1}
            max={25}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, positionSizePct: v })}
          />
          <ParamSlider
            id={maxTradesId}
            label="Max daily trades"
            hint="Hard limit on how many buys can execute in a single trading day. It does not cap how many signals a cycle generates. Lower values slow turnover and reduce fee drag; higher values allow more fills."
            value={strategyDraft.maxDailyTrades}
            min={1}
            max={20}
            format={(v) => String(v)}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, maxDailyTrades: v })}
          />
          <ParamSlider
            id={minConfId}
            label="Min confidence"
            hint="Signals below this model confidence are blocked. Raise it to trade only high-conviction ideas; lower it to accept more borderline recommendations."
            value={strategyDraft.minConfidenceToTrade}
            min={40}
            max={95}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, minConfidenceToTrade: v })}
          />
          <ParamSlider
            id={maxDdId}
            label="Max daily drawdown"
            hint="If today’s equity drop reaches this level, new buys are blocked for the rest of the day. Protects the book after a sharp session loss."
            value={strategyDraft.maxDailyDrawdownPct}
            min={1}
            max={15}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, maxDailyDrawdownPct: v })}
          />
          <ParamSlider
            id={stopLossId}
            label="Stop loss"
            hint="Enforced each cycle: sell an open position when last price is this far below average cost."
            value={strategyDraft.stopLossPct}
            min={1}
            max={25}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, stopLossPct: v })}
          />
          <ParamSlider
            id={takeProfitId}
            label="Take profit"
            hint="Enforced each cycle: sell an open position when last price is this far above average cost."
            value={strategyDraft.takeProfitPct}
            min={2}
            max={40}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, takeProfitPct: v })}
          />
        </div>

        <div className="btn-row">
          <button type="button" className="btn btn-primary" disabled={strategyBusy} onClick={() => void saveStrategy()}>
            {strategyBusy && <IconSpinner />}
            {strategyBusy ? 'Saving…' : 'Save strategy'}
          </button>
        </div>
      </section>

      <section className="panel" aria-labelledby="automation-heading">
        <h2 id="automation-heading">Automation</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Control whether Pulsar runs ingest → signals → execution cycles on a timer during NGX market hours.
        </p>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label" htmlFor={autoCycleId}>Run cycles automatically</label>
            <p className="toggle-row__hint">
              When on, the app schedules full trading cycles in the background. When off, cycles only run when you trigger them from Home.
            </p>
          </div>
          <button
            id={autoCycleId}
            type="button"
            role="switch"
            aria-checked={autoCycleEnabled}
            className={`toggle ${autoCycleEnabled ? 'is-on' : ''}`}
            disabled={cycleBusy}
            onClick={() => setAutoCycleEnabled((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <div className="param-slider-grid" style={{ marginTop: 20 }}>
          <ParamSlider
            id={cycleIntervalId}
            label="Cycle frequency"
            hint="Minutes between automatic cycles while the market is open (or in the post-close window). Shorter intervals react faster but use more LLM calls."
            value={autoCycleMinutes}
            min={5}
            max={120}
            step={5}
            format={(v) => `${v} min`}
            disabled={cycleBusy || !autoCycleEnabled}
            onChange={setAutoCycleMinutes}
          />
        </div>

        <div className="btn-row">
          <button type="button" className="btn btn-primary" disabled={cycleBusy} onClick={() => void saveAutoCycle()}>
            {cycleBusy && <IconSpinner />}
            {cycleBusy ? 'Saving…' : 'Save automation'}
          </button>
        </div>
      </section>

      <section className="panel">
        <h2>LLM</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Current provider configuration. Change credentials only when you need to rotate a key.
        </p>
        <dl className="summary-row" style={{ marginTop: 16 }}>
          <dt>Provider</dt>
          <dd>{providerLabel}</dd>
          <dt>Model</dt>
          <dd>{llmStatus.model || '—'}</dd>
          {llmStatus.baseUrl ? (
            <>
              <dt>Base URL</dt>
              <dd>{llmStatus.baseUrl}</dd>
            </>
          ) : null}
          <dt>API key</dt>
          <dd>
            {llmStatus.configured
              ? (llmStatus.maskedKey || '••••••••')
              : 'Not configured'}
          </dd>
        </dl>
        <div className="btn-row">
          <button type="button" className="btn btn-ghost" onClick={openLlmModal}>
            Change LLM settings
          </button>
        </div>
      </section>

      <section className="panel panel--danger" aria-labelledby="danger-heading">
        <h2 id="danger-heading">Danger zone</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Reset local Pulsar data: SQLite database (portfolio, signals, trades, history), settings,
          and OS keychain secrets (Pulse, LLM, Wealth). You will go through onboarding again.
          This does not affect your Coronation Wealth or NGX Pulse accounts.
        </p>
        {dataDir ? (
          <p className="muted" style={{ fontSize: 12, lineHeight: 1.45 }}>
            Data folder: <span className="mono">{dataDir}</span>
          </p>
        ) : null}
        <label className="label" htmlFor="reset-confirm">
          Type RESET to confirm
        </label>
        <input
          id="reset-confirm"
          className="input"
          autoComplete="off"
          value={resetConfirm}
          disabled={resetBusy}
          onChange={(e) => setResetConfirm(e.target.value)}
          placeholder="RESET"
        />
        <div className="btn-row">
          <button
            type="button"
            className="btn btn-danger"
            disabled={resetBusy || resetConfirm.trim() !== 'RESET'}
            onClick={() => {
              void (async () => {
                setResetBusy(true);
                try {
                  await api('reset_local_data');
                  toast.success('Local data cleared. Reloading…', 'Reset');
                  window.location.hash = '#/onboarding';
                  window.location.reload();
                } catch (e) {
                  toast.error(String(e), 'Reset failed');
                  setResetBusy(false);
                }
              })();
            }}
          >
            {resetBusy ? <IconSpinner /> : null}
            {resetBusy ? 'Resetting…' : 'Reset local data'}
          </button>
        </div>
      </section>

      {llmModalOpen && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) closeLlmModal();
          }}
        >
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby={titleId}
          >
            <div className="modal__header">
              <div>
                <h2 id={titleId}>Change LLM settings</h2>
                <p className="muted" style={{ margin: '6px 0 0', fontSize: 13 }}>
                  Updates are verified against the provider before they are kept.
                </p>
              </div>
              <button type="button" className="modal__close" aria-label="Close" onClick={closeLlmModal} disabled={modalBusy}>
                ×
              </button>
            </div>

            <label className="label" htmlFor={providerId}>Provider</label>
            <select
              id={providerId}
              ref={firstFieldRef}
              className="input"
              disabled={modalBusy}
              value={draftProvider}
              onChange={(e) => setDraftProvider(e.target.value)}
            >
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
              <option value="openrouter">OpenRouter</option>
            </select>

            <label className="label" htmlFor={modelId}>Model</label>
            <input
              id={modelId}
              className="input"
              disabled={modalBusy}
              value={draftModel}
              onChange={(e) => setDraftModel(e.target.value)}
            />

            <label className="label" htmlFor={baseId}>Base URL (optional)</label>
            <input
              id={baseId}
              className="input"
              disabled={modalBusy}
              value={draftBaseUrl}
              onChange={(e) => setDraftBaseUrl(e.target.value)}
              placeholder="Leave blank for provider default"
            />

            <label className="label" htmlFor={keyId}>API key</label>
            <input
              id={keyId}
              className="input"
              type="password"
              autoComplete="off"
              disabled={modalBusy}
              value={draftApiKey}
              onChange={(e) => setDraftApiKey(e.target.value)}
              placeholder="Paste new API key"
            />

            {modalStatus && (
              <div
                className={`banner ${modalStatus.includes('verifying') || modalStatus.includes('Saving') ? 'banner-muted' : 'banner-bad'}`}
                style={{ marginTop: 16 }}
                role="status"
              >
                {modalStatus}
              </div>
            )}

            <div className="btn-row">
              <button type="button" className="btn btn-ghost" disabled={modalBusy} onClick={closeLlmModal}>
                Cancel
              </button>
              <button type="button" className="btn btn-primary" disabled={modalBusy} onClick={() => void saveLlmFromModal()}>
                {modalBusy && <IconSpinner />}
                {modalBusy ? 'Verifying…' : 'Save & verify'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
