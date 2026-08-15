import { useEffect, useId, useRef, useState } from 'react';
import bambooLogo from '../assets/bamboo.webp';
import wealthLogo from '../assets/wealth.webp';
import { IconLogout, IconSpinner } from '../components/Icons';
import { api, type AppSettings } from '../lib/api';
import { formatNaira } from '../lib/format';
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

type BrokerListItem = {
  id: 'wealth' | 'bamboo' | string;
  name: string;
  available: boolean;
  connected: boolean;
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

const BROKER_LOGOS: Record<string, string> = {
  wealth: wealthLogo,
  bamboo: bambooLogo,
};

const DEFAULT_BROKERS: BrokerListItem[] = [
  { id: 'wealth', name: 'Coronation Wealth', available: true, connected: false },
  { id: 'bamboo', name: 'Bamboo', available: true, connected: false },
];

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
  const { logout, activeModule, setActiveModule } = useSession();
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
  const [brokers, setBrokers] = useState<BrokerListItem[]>([]);
  const [selectedBroker, setSelectedBroker] = useState('wealth');
  const [wealthEmail, setWealthEmail] = useState('');
  const [wealthPassword, setWealthPassword] = useState('');
  const [wealth2fa, setWealth2fa] = useState('');
  const [wealthTempToken, setWealthTempToken] = useState<string | null>(null);
  const [wealthBusy, setWealthBusy] = useState(false);
  const [bambooPhone, setBambooPhone] = useState('');
  const [bambooPassword, setBambooPassword] = useState('');
  const [bambooPin, setBambooPin] = useState('');
  const [brokerModalOpen, setBrokerModalOpen] = useState(false);
  const brokerModalTitleId = useId();
  const [strategyDraft, setStrategyDraft] = useState<StrategyDraft | null>(null);
  const [autoCycleEnabled, setAutoCycleEnabled] = useState(false);
  const [autoCycleMinutes, setAutoCycleMinutes] = useState(30);
  const [liveTradingEnabled, setLiveTradingEnabled] = useState(false);
  const [scheduledLiveAuthorized, setScheduledLiveAuthorized] = useState(false);
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
  const [moduleConfirm, setModuleConfirm] = useState<'stocks' | 'crypto' | null>(null);
  const [moduleBusy, setModuleBusy] = useState(false);

  async function refresh() {
    const [s, llm, strategy] = await Promise.all([
      api<AppSettings>('settings_get'),
      api<LlmStatus>('llm_status'),
      api<StrategyRecord>('get_strategy'),
    ]);
    setSettings(s);
    setActiveModule(s.activeModule === 'crypto' ? 'crypto' : 'stocks');
    setLlmStatus(llm);
    setStrategyDraft(draftFromStrategy(strategy));
    setAutoCycleEnabled(!!s.autoCycleEnabled);
    setAutoCycleMinutes(clamp(Math.round(s.autoCycleIntervalMinutes || 30), 5, 120));
    setLiveTradingEnabled(!!s.liveTradingEnabled);
    setScheduledLiveAuthorized(!!s.scheduledLiveAuthorized);
    setSelectedBroker(s.selectedBroker || 'wealth');
    try {
      setBrokers(await api<BrokerListItem[]>('broker_list'));
    } catch {
      setBrokers([
        { id: 'wealth', name: 'Coronation Wealth', available: true, connected: !!s.wealthConnected },
        { id: 'bamboo', name: 'Bamboo', available: true, connected: !!s.bambooConnected },
      ]);
    }
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
      const cmd = (s.selectedBroker || 'wealth') === 'bamboo' ? 'bamboo_profile' : 'wealth_profile';
      const w = await api<WealthProfile>(cmd);
      setWealth(w);
      if ((s.selectedBroker || 'wealth') === 'bamboo') {
        if (s.bambooPhone) setBambooPhone(s.bambooPhone);
      } else if (w.email) {
        setWealthEmail(w.email);
      }
    } catch {
      setWealth({
        ok: false,
        connected: false,
        tradingVerified: false,
        tradingMode: 'sandbox',
        baseUrl: '',
        message: 'Broker status unavailable.',
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

  useEffect(() => {
    if (!brokerModalOpen) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && !wealthBusy) closeBrokerModal();
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [brokerModalOpen, wealthBusy]);

  function closeBrokerModal() {
    if (wealthBusy) return;
    setBrokerModalOpen(false);
    setWealthTempToken(null);
    setWealth2fa('');
    setWealthPassword('');
    setBambooPassword('');
    setBambooPin('');
  }

  function brokerIsConnected(id: string, s: AppSettings | null) {
    if (id === 'bamboo') return Boolean(s?.bambooConnected);
    return Boolean(s?.wealthConnected);
  }

  async function selectBroker(next: string) {
    if (wealthBusy) return;
    const switching = next !== selectedBroker;
    if (switching) {
      setWealthBusy(true);
      try {
        const updated = await api<AppSettings>('set_selected_broker', { brokerId: next });
        setSettings(updated);
        setSelectedBroker(updated.selectedBroker || next);
        toast.success(
          next === 'bamboo'
            ? 'Bamboo selected as the live broker.'
            : 'Coronation Wealth selected as the live broker.',
          'Live broker',
        );
        await refresh();
        if (!brokerIsConnected(next, updated)) {
          setBrokerModalOpen(true);
        }
      } catch (err) {
        toast.error(String(err), 'Could not change broker');
      } finally {
        setWealthBusy(false);
      }
      return;
    }
    if (!brokerIsConnected(next, settings) && !wealth?.connected) {
      setBrokerModalOpen(true);
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
          liveTradingEnabled,
          scheduledLiveAuthorized,
        },
      });
      setSettings({
        ...settings,
        autoCycleEnabled,
        autoCycleIntervalMinutes: minutes,
        liveTradingEnabled,
        scheduledLiveAuthorized,
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

  async function switchModule(next: 'stocks' | 'crypto') {
    if (!settings) return;
    const current = activeModule === 'crypto' || settings.activeModule === 'crypto' ? 'crypto' : 'stocks';
    if (next === current) {
      setModuleConfirm(null);
      return;
    }
    if (moduleConfirm !== next) {
      setModuleConfirm(next);
      return;
    }
    setModuleBusy(true);
    try {
      const payload = {
        ...settings,
        activeModule: next,
        autoCycleEnabled: false,
        liveTradingEnabled: next === 'crypto' ? false : settings.liveTradingEnabled,
        scheduledLiveAuthorized: next === 'crypto' ? false : settings.scheduledLiveAuthorized,
      };
      await api('settings_set', { settings: payload });
      setSettings(payload);
      setActiveModule(next);
      setAutoCycleEnabled(false);
      if (next === 'crypto') {
        setLiveTradingEnabled(false);
        setScheduledLiveAuthorized(false);
      }
      setModuleConfirm(null);
      toast.success(next === 'crypto' ? 'Crypto sandbox active.' : 'Stocks workspace active.', 'Module');
      try {
        await refresh();
      } catch {
        /* local state already applied */
      }
    } catch (e) {
      toast.error(String(e), 'Could not switch module');
      setModuleConfirm(null);
    } finally {
      setModuleBusy(false);
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
  const isCrypto = activeModule === 'crypto' || settings.activeModule === 'crypto';

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Settings</h1>
          <p>Profile, strategy risk parameters, and LLM credentials.</p>
        </div>
      </header>

      <section className="panel" aria-labelledby="module-heading" style={{ marginBottom: 20 }}>
        <h2 id="module-heading">Market module</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Stocks uses NGX Pulse quotes and the NGN sandbox. Crypto uses public USDT spot quotes and a separate sandbox book. Pulse login stays required for both. Switching stops auto-cycle.
        </p>
        <div className="broker-grid" role="radiogroup" aria-labelledby="module-heading">
          <button
            type="button"
            role="radio"
            aria-checked={!isCrypto}
            className={`broker-card${!isCrypto ? ' is-selected' : ''}`}
            disabled={moduleBusy}
            onClick={() => void switchModule('stocks')}
          >
            <span className="broker-card__name">Stocks</span>
            <span className="broker-card__meta">NGX · NGN sandbox</span>
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={isCrypto}
            className={`broker-card${isCrypto ? ' is-selected' : ''}`}
            disabled={moduleBusy}
            onClick={() => void switchModule('crypto')}
          >
            <span className="broker-card__name">Crypto</span>
            <span className="broker-card__meta">USDT · sandbox only · 24/7</span>
          </button>
        </div>
        {moduleConfirm ? (
          <div className="banner banner-warn" style={{ marginTop: 16 }} role="status">
            <p style={{ margin: '0 0 12px', fontSize: 13, lineHeight: 1.45 }}>
              {moduleConfirm === 'crypto'
                ? 'Switch to Crypto? Auto-cycle will stop. Crypto is sandbox-only with public USDT quotes — live brokers are disabled.'
                : 'Switch to Stocks? Auto-cycle will stop. The NGX stocks sandbox (and any connected live broker) will be used.'}
            </p>
            <div className="btn-row" style={{ margin: 0 }}>
              <button
                type="button"
                className="btn btn-ghost"
                disabled={moduleBusy}
                onClick={() => setModuleConfirm(null)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={moduleBusy}
                onClick={() => void switchModule(moduleConfirm)}
              >
                {moduleBusy ? <IconSpinner /> : null}
                {moduleBusy ? 'Switching…' : `Confirm ${moduleConfirm === 'crypto' ? 'Crypto' : 'Stocks'}`}
              </button>
            </div>
          </div>
        ) : null}
      </section>

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

      {!isCrypto ? (
      <>
      <section className="panel" aria-labelledby="broker-heading" style={{ marginTop: 20 }}>
        <h2 id="broker-heading">Live broker</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          One live broker at a time. Pulse stays the market-data source. Click a broker to select it
          and connect. Without a connected live broker, Pulsar runs in sandbox mode.
        </p>
        <div className="broker-grid" role="radiogroup" aria-labelledby="broker-heading">
          {(brokers.length ? brokers : DEFAULT_BROKERS).map((b) => {
            const selected = selectedBroker === b.id;
            return (
              <button
                key={b.id}
                type="button"
                role="radio"
                aria-checked={selected}
                className={`broker-card${selected ? ' is-selected' : ''}`}
                disabled={wealthBusy}
                onClick={() => void selectBroker(b.id)}
              >
                {BROKER_LOGOS[b.id] ? (
                  <img src={BROKER_LOGOS[b.id]} alt="" className="broker-card__logo" />
                ) : (
                  <span className="broker-card__logo broker-card__logo--fallback" aria-hidden>
                    {b.name.slice(0, 1)}
                  </span>
                )}
                <span className="broker-card__name">{b.name}</span>
                <span className="broker-card__meta">
                  {b.connected ? 'Connected' : b.available ? 'Available' : 'Coming soon'}
                </span>
              </button>
            );
          })}
        </div>
      </section>

      {wealth?.connected ? (
      <section className="panel" aria-labelledby="wealth-heading" style={{ marginTop: 20 }}>
        <h2 id="wealth-heading">{selectedBroker === 'bamboo' ? 'Bamboo' : 'Coronation Wealth'}</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          {selectedBroker === 'bamboo'
            ? 'Cash and lots come from Bamboo. Approved signals place real NGX market orders when live trading is on.'
            : 'Cash and portfolio come from Wealth. Approved signals place real market orders when live trading is on.'}
        </p>
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
              {wealth.brokerageBalance != null ? formatNaira(wealth.brokerageBalance) : '—'}
            </dd>
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
                  if (selectedBroker === 'bamboo') {
                    await api('bamboo_logout');
                    setBambooPassword('');
                    setBambooPin('');
                    toast.success('Bamboo account disconnected. Sandbox mode restored.', 'Bamboo');
                  } else {
                    await api('wealth_logout');
                    setWealthTempToken(null);
                    setWealthPassword('');
                    setWealth2fa('');
                    toast.success('Wealth account disconnected. Sandbox mode restored.', 'Wealth');
                  }
                  await refresh();
                } catch (e) {
                  toast.error(String(e), selectedBroker === 'bamboo' ? 'Bamboo logout failed' : 'Wealth logout failed');
                } finally {
                  setWealthBusy(false);
                }
              })();
            }}
          >
            {wealthBusy ? <IconSpinner /> : null}
            Disconnect {selectedBroker === 'bamboo' ? 'Bamboo' : 'Wealth'}
          </button>
        </div>
      </section>
      ) : null}
      </>
      ) : (
      <section className="panel" aria-labelledby="broker-heading" style={{ marginTop: 20 }}>
        <h2 id="broker-heading">Live broker</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Crypto is sandbox-only. Public USDT quotes feed signals and simulated fills. Live crypto brokers are not available in this version.
        </p>
      </section>
      )}

      <section className="panel" aria-labelledby="strategy-heading">
        <h2 id="strategy-heading">Strategy</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          {isCrypto
            ? 'Risk and sizing for the crypto USDT sandbox. The universe is the curated USDT spot list.'
            : wealth?.tradingMode === 'live'
            ? `Risk and sizing for live ${selectedBroker === 'bamboo' ? 'Bamboo' : 'Wealth'} orders. The trading universe is all active NGX instruments from Pulse.`
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
          Control whether Pulsar runs ingest → signals → execution cycles on a timer
          {isCrypto ? ' (crypto is 24/7).' : ' during NGX market hours.'}
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

        {!isCrypto ? (
        <>
        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label">Enable live trading</label>
            <p className="toggle-row__hint">
              Required before any Wealth order is submitted. Manual live cycles still ask for a confirmation token.
            </p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={liveTradingEnabled}
            className={`toggle ${liveTradingEnabled ? 'is-on' : ''}`}
            disabled={cycleBusy}
            onClick={() => setLiveTradingEnabled((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label">Authorize scheduled live cycles</label>
            <p className="toggle-row__hint">
              Recurring live execution for the scheduler. Revoke anytime; the next cycle is blocked immediately.
            </p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={scheduledLiveAuthorized}
            className={`toggle ${scheduledLiveAuthorized ? 'is-on' : ''}`}
            disabled={cycleBusy || !liveTradingEnabled}
            onClick={() => setScheduledLiveAuthorized((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>
        </>
        ) : null}

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

      {brokerModalOpen && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) closeBrokerModal();
          }}
        >
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby={brokerModalTitleId}
          >
            <div className="modal__header">
              <div>
                <h2 id={brokerModalTitleId}>
                  {selectedBroker === 'bamboo' ? 'Connect Bamboo' : 'Connect Wealth'}
                </h2>
                <p className="muted" style={{ margin: '6px 0 0', fontSize: 13 }}>
                  {selectedBroker === 'bamboo'
                    ? 'Sign in with your Bamboo phone and password. A CSCS-ready NGX account is required for live orders.'
                    : 'Sign in with your Coronation Wealth email and password. A trading-verified account is required for live orders.'}
                </p>
              </div>
              <button
                type="button"
                className="modal__close"
                aria-label="Close"
                onClick={closeBrokerModal}
                disabled={wealthBusy}
              >
                ×
              </button>
            </div>

            {selectedBroker === 'bamboo' ? (
              <div className="form-stack">
                <label>
                  <span>Phone number</span>
                  <input
                    type="tel"
                    autoComplete="tel"
                    value={bambooPhone}
                    onChange={(e) => setBambooPhone(e.target.value)}
                    placeholder="08012345678"
                    disabled={wealthBusy}
                  />
                </label>
                <label>
                  <span>Password</span>
                  <input
                    type="password"
                    autoComplete="current-password"
                    value={bambooPassword}
                    onChange={(e) => setBambooPassword(e.target.value)}
                    placeholder="Bamboo app password"
                    disabled={wealthBusy}
                  />
                </label>
                <label>
                  <span>Transaction PIN (optional)</span>
                  <input
                    type="password"
                    inputMode="numeric"
                    autoComplete="off"
                    maxLength={4}
                    value={bambooPin}
                    onChange={(e) => setBambooPin(e.target.value.replace(/\D/g, '').slice(0, 4))}
                    placeholder="4-digit PIN"
                    disabled={wealthBusy}
                  />
                </label>
                <div className="btn-row">
                  <button type="button" className="btn btn-ghost" disabled={wealthBusy} onClick={closeBrokerModal}>
                    Cancel
                  </button>
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={wealthBusy || !bambooPhone.trim() || !bambooPassword}
                    onClick={() => {
                      void (async () => {
                        setWealthBusy(true);
                        try {
                          const result = await api<{ ok: boolean; connected: boolean; message: string }>(
                            'bamboo_login',
                            {
                              phoneNumber: bambooPhone.trim(),
                              password: bambooPassword,
                              transactionPin: bambooPin.trim() || undefined,
                            },
                          );
                          if (!result.ok) {
                            toast.error(result.message, 'Bamboo login');
                            return;
                          }
                          setBambooPassword('');
                          setBambooPin('');
                          setBrokerModalOpen(false);
                          toast.success(result.message, 'Bamboo');
                          await refresh();
                        } catch (e) {
                          toast.error(String(e), 'Bamboo login');
                        } finally {
                          setWealthBusy(false);
                        }
                      })();
                    }}
                  >
                    {wealthBusy ? <IconSpinner /> : null}
                    Connect Bamboo
                  </button>
                </div>
              </div>
            ) : wealthTempToken ? (
              <div className="form-stack">
                <label>
                  <span>2FA code</span>
                  <input
                    type="text"
                    inputMode="numeric"
                    autoComplete="one-time-code"
                    value={wealth2fa}
                    onChange={(e) => setWealth2fa(e.target.value)}
                    placeholder="Enter authentication code"
                    disabled={wealthBusy}
                  />
                </label>
                <div className="btn-row">
                  <button
                    type="button"
                    className="btn btn-ghost"
                    disabled={wealthBusy}
                    onClick={() => {
                      setWealthTempToken(null);
                      setWealth2fa('');
                    }}
                  >
                    Back
                  </button>
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
                          setBrokerModalOpen(false);
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
                </div>
              </div>
            ) : (
              <div className="form-stack">
                <label>
                  <span>Email</span>
                  <input
                    type="email"
                    autoComplete="username"
                    value={wealthEmail}
                    onChange={(e) => setWealthEmail(e.target.value)}
                    placeholder="you@example.com"
                    disabled={wealthBusy}
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
                    disabled={wealthBusy}
                  />
                </label>
                <div className="btn-row">
                  <button type="button" className="btn btn-ghost" disabled={wealthBusy} onClick={closeBrokerModal}>
                    Cancel
                  </button>
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
                          setBrokerModalOpen(false);
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
              </div>
            )}
          </div>
        </div>
      )}

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
