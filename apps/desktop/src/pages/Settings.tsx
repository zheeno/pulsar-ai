import { Fragment, useEffect, useId, useRef, useState } from 'react';
import bambooLogo from '../assets/bamboo.webp';
import bushaLogo from '../assets/busha.webp';
import wealthLogo from '../assets/wealth.webp';
import { IconLogout, IconSpinner } from '../components/Icons';
import {
  draftFromLlmTemperature,
  formatLlmTemperatureLabel,
  LlmTemperatureControl,
  llmTemperatureFromDraft,
} from '../components/LlmTemperatureControl';
import { api, type AppSettings } from '../lib/api';
import {
  authenticate as authBridgeAuthenticate,
  listSessions,
  onExpired,
  onExpiring,
  revoke as authBridgeRevoke,
  type AuthSessionStatus,
} from '../lib/auth-bridge';
import { formatNaira } from '../lib/format';
import {
  notificationSoundsEnabled,
  playNotificationSound,
  setNotificationSoundsEnabled,
} from '../lib/notify-sound';
import { useSession } from '../lib/session';
import { useToast } from '../lib/toast';

type LlmStatus = {
  provider: string;
  model: string;
  baseUrl?: string | null;
  temperature?: number | null;
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
  hasSession?: boolean;
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
  max_daily_trades?: number;
  stop_loss_pct: number;
  take_profit_pct?: number | null;
  min_confidence_to_trade: number;
  max_daily_drawdown_pct: number;
  position_size_pct?: number;
  cycle_budget_pct: number;
  time_stop_hours?: number;
  partial_tp_fraction?: number;
};

type StrategyDraft = {
  maxPositionPct: number;
  cycleBudgetPct: number;
  minConfidenceToTrade: number;
  maxDailyDrawdownPct: number;
  stopLossPct: number;
  takeProfitPct: number;
  timeStopHours: number;
  partialTpPct: number;
};

type JournalRow = {
  bucket: string;
  n: number;
  hitRate: number;
  avgPnl?: number | null;
  avgReturnPct?: number | null;
};

const BROKER_LOGOS: Record<string, string> = {
  wealth: wealthLogo,
  bamboo: bambooLogo,
  busha: bushaLogo,
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
    cycleBudgetPct: toPct(s.cycle_budget_pct ?? 0.2, 5, 100),
    minConfidenceToTrade: toPct(s.min_confidence_to_trade, 40, 95),
    maxDailyDrawdownPct: toPct(s.max_daily_drawdown_pct, 1, 100),
    stopLossPct: toPct(s.stop_loss_pct, 1, 25),
    takeProfitPct: toPct(s.take_profit_pct ?? 0.1, 2, 40),
    timeStopHours: clamp(Math.round(s.time_stop_hours ?? 24), 0, 168),
    partialTpPct: clamp(Math.round((s.partial_tp_fraction ?? 1) * 100), 10, 100),
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
  const minConfId = useId();
  const maxDdId = useId();
  const stopLossId = useId();
  const takeProfitId = useId();
  const timeStopId = useId();
  const partialTpId = useId();
  const autoCycleId = useId();
  const cycleIntervalId = useId();
  const haltBuysId = useId();
  const flattenId = useId();
  const launchId = useId();
  const maxActionsId = useId();
  const maxNotionalId = useId();
  const retainLogsId = useId();
  const soundsId = useId();
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
  const [haltNewBuys, setHaltNewBuys] = useState(false);
  const [flattenArmed, setFlattenArmed] = useState(false);
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [appDownAck, setAppDownAck] = useState(false);
  const [maxLiveActions, setMaxLiveActions] = useState(10);
  const [maxLiveNotional, setMaxLiveNotional] = useState(500_000);
  const [retainRawLlmLogs, setRetainRawLlmLogs] = useState(false);
  const [journal, setJournal] = useState<JournalRow[]>([]);
  const [busy, setBusy] = useState(false);
  const [strategyBusy, setStrategyBusy] = useState(false);
  const [cycleBusy, setCycleBusy] = useState(false);

  const [llmModalOpen, setLlmModalOpen] = useState(false);
  const [draftProvider, setDraftProvider] = useState('openai');
  const [draftModel, setDraftModel] = useState('');
  const [draftBaseUrl, setDraftBaseUrl] = useState('');
  const [draftUseProviderDefault, setDraftUseProviderDefault] = useState(true);
  const [draftTemperature, setDraftTemperature] = useState(0.7);
  const [draftApiKey, setDraftApiKey] = useState('');
  const [modalStatus, setModalStatus] = useState('');
  const [modalBusy, setModalBusy] = useState(false);
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [resetConfirm, setResetConfirm] = useState('');
  const [resetBusy, setResetBusy] = useState(false);
  const [authBridgeAccounts, setAuthBridgeAccounts] = useState<AuthSessionStatus[]>([]);
  const [authBridgeBusy, setAuthBridgeBusy] = useState<string | null>(null);
  const [notificationSounds, setNotificationSounds] = useState(() => notificationSoundsEnabled());
  const [bushaCash, setBushaCash] = useState<number | null>(null);
  const [confirmKind, setConfirmKind] = useState<'logout' | 'busha' | 'wealth' | 'bamboo' | null>(
    null,
  );
  const confirmTitleId = useId();
  const [confirmBusy, setConfirmBusy] = useState(false);

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
    setLiveTradingEnabled(!!s.liveTradingEnabled);
    setHaltNewBuys(!!s.haltNewBuys);
    setFlattenArmed(!!s.flattenOnDrawdownArmed);
    setLaunchAtLogin(!!s.launchAtLogin);
    setMaxLiveActions(clamp(Math.round(s.maxLiveActions || 10), 1, 40));
    setMaxLiveNotional(clamp(Math.round(s.maxLiveNotional || 500_000), 1_000, 50_000_000));
    setRetainRawLlmLogs(!!s.retainRawLlmLogs);
    if (s.launchAtLogin) setAppDownAck(true);
    try {
      setJournal(await api<JournalRow[]>('confidence_journal'));
    } catch {
      setJournal([]);
    }
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
      setAuthBridgeAccounts(await listSessions());
    } catch {
      setAuthBridgeAccounts([]);
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
    if (s.assetClass === 'crypto' || s.cryptoSession === 'expiredNeedsReconnect') {
      try {
        const book = await api<{
          portfolio?: { cash_balance?: number };
          total_equity?: number;
        }>('portfolio_default');
        setBushaCash(
          book.portfolio?.cash_balance != null ? Number(book.portfolio.cash_balance) : null,
        );
      } catch {
        setBushaCash(null);
      }
    } else {
      setBushaCash(null);
    }
  }

  useEffect(() => {
    refresh().catch(() => {});
  }, []);

  useEffect(() => {
    let unlistenExpired: (() => void) | undefined;
    let unlistenExpiring: (() => void) | undefined;
    void onExpired((event) => {
      if (event.brokerId !== 'busha') {
        toast.warning(
          `${event.brokerId} session expired. Reconnect to resume trading.`,
          'Live broker',
        );
      }
      void listSessions()
        .then(setAuthBridgeAccounts)
        .catch(() => {});
      void refresh().catch(() => {});
    }).then((fn) => {
      unlistenExpired = fn;
    });
    void onExpiring(() => {
      void listSessions()
        .then(setAuthBridgeAccounts)
        .catch(() => {});
    }).then((fn) => {
      unlistenExpiring = fn;
    });
    return () => {
      unlistenExpired?.();
      unlistenExpiring?.();
    };
  }, [toast]);

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
    const tempDraft = draftFromLlmTemperature(
      llmStatus?.temperature ?? settings?.llmTemperature ?? null,
    );
    setDraftProvider(llmStatus?.provider || settings?.llmProvider || 'openai');
    setDraftModel(llmStatus?.model || settings?.llmModel || '');
    setDraftBaseUrl(llmStatus?.baseUrl || settings?.llmBaseUrl || '');
    setDraftUseProviderDefault(tempDraft.useProviderDefault);
    setDraftTemperature(tempDraft.temperature);
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
    const needsNewKey = !llmStatus?.configured;
    if (needsNewKey && !draftApiKey.trim()) {
      setModalStatus('Enter an API key to configure LLM credentials.');
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
          llmTemperature: llmTemperatureFromDraft(draftUseProviderDefault, draftTemperature),
          llmConfigured: true,
        },
        ...(draftApiKey.trim() ? { llmApiKey: draftApiKey.trim() } : {}),
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
          cycleBudgetPct: strategyDraft.cycleBudgetPct / 100,
          minConfidenceToTrade: strategyDraft.minConfidenceToTrade / 100,
          maxDailyDrawdownPct: strategyDraft.maxDailyDrawdownPct / 100,
          stopLossPct: strategyDraft.stopLossPct / 100,
          takeProfitPct: strategyDraft.takeProfitPct / 100,
          timeStopHours: strategyDraft.timeStopHours,
          partialTpFraction: strategyDraft.partialTpPct / 100,
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

  useEffect(() => {
    if (!confirmKind) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && !confirmBusy) closeConfirmModal();
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [confirmKind, confirmBusy]);

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

  async function connectAuthBridge(brokerId: string) {
    setAuthBridgeBusy(brokerId);
    try {
      const session = await authBridgeAuthenticate(brokerId);
      setAuthBridgeAccounts((prev) => {
        const rest = prev.filter((a) => a.brokerId !== session.brokerId);
        return [...rest, session];
      });
      toast.success(`${session.displayName} connected.`, 'Live broker');
      await refresh().catch(() => {});
    } catch (err) {
      toast.error(String(err), 'Could not connect');
    } finally {
      setAuthBridgeBusy(null);
    }
  }

  async function disconnectAuthBridge(brokerId: string) {
    setAuthBridgeBusy(brokerId);
    try {
      const session = await authBridgeRevoke(brokerId);
      setAuthBridgeAccounts((prev) =>
        prev.map((a) => (a.brokerId === session.brokerId ? session : a)),
      );
      toast.success('Busha disconnected. NGX brokers are available again.', 'Live broker');
      await refresh().catch(() => {});
    } catch (err) {
      toast.error(String(err), 'Could not disconnect');
    } finally {
      setAuthBridgeBusy(null);
    }
  }

  async function disconnectNgxBroker() {
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
      toast.error(
        String(e),
        selectedBroker === 'bamboo' ? 'Bamboo logout failed' : 'Wealth logout failed',
      );
    } finally {
      setWealthBusy(false);
    }
  }

  function closeConfirmModal() {
    if (confirmBusy) return;
    setConfirmKind(null);
  }

  async function runConfirmedAction() {
    if (!confirmKind || confirmBusy) return;
    setConfirmBusy(true);
    try {
      if (confirmKind === 'logout') {
        setBusy(true);
        toast.info('Logging out…');
        try {
          await logout();
        } catch (e) {
          toast.error(String(e), 'Logout failed');
          setBusy(false);
        }
      } else if (confirmKind === 'busha') {
        await disconnectAuthBridge('busha');
      } else {
        await disconnectNgxBroker();
      }
      setConfirmKind(null);
    } finally {
      setConfirmBusy(false);
    }
  }

  const confirmCopy =
    confirmKind === 'logout'
      ? {
          title: 'Log out of Pulsar?',
          body: 'This ends your NGX Pulse session and returns you to onboarding. Local portfolio data stays on this device.',
          confirmLabel: 'Log out',
          danger: true,
        }
      : confirmKind === 'busha'
        ? {
            title: 'Disconnect Busha?',
            body: 'Live crypto trading stops and the app leaves crypto mode. NGX brokers (Wealth / Bamboo) become available again.',
            confirmLabel: 'Disconnect Busha',
            danger: false,
          }
        : confirmKind === 'bamboo'
          ? {
              title: 'Disconnect Bamboo?',
              body: 'Live Bamboo orders stop and Pulsar returns to sandbox for NGX until you reconnect a broker.',
              confirmLabel: 'Disconnect Bamboo',
              danger: false,
            }
          : confirmKind === 'wealth'
            ? {
                title: 'Disconnect Coronation Wealth?',
                body: 'Live Wealth orders stop and Pulsar returns to sandbox for NGX until you reconnect a broker.',
                confirmLabel: 'Disconnect Wealth',
                danger: false,
              }
            : null;

  async function saveAutoCycle() {
    if (!settings) return;
    setCycleBusy(true);
    toast.info('Saving automation…');
    try {
      const minutes = clamp(autoCycleMinutes, 5, 120);
      const actions = clamp(maxLiveActions, 1, 40);
      const notional = clamp(maxLiveNotional, 1_000, 50_000_000);
      await api('settings_set', {
        settings: {
          ...settings,
          autoCycleEnabled,
          autoCycleIntervalMinutes: minutes,
          liveTradingEnabled,
          haltNewBuys,
          flattenOnDrawdownArmed: flattenArmed,
          launchAtLogin,
          maxLiveActions: actions,
          maxLiveNotional: notional,
          retainRawLlmLogs,
        },
      });
      setSettings({
        ...settings,
        autoCycleEnabled,
        autoCycleIntervalMinutes: minutes,
        liveTradingEnabled,
        haltNewBuys,
        flattenOnDrawdownArmed: flattenArmed,
        launchAtLogin,
        maxLiveActions: actions,
        maxLiveNotional: notional,
        retainRawLlmLogs,
      });
      setAutoCycleMinutes(minutes);
      setMaxLiveActions(actions);
      setMaxLiveNotional(notional);
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
  const bushaAccount =
    authBridgeAccounts.find((a) => a.brokerId === 'busha') ?? {
      brokerId: 'busha',
      displayName: 'Busha',
      status: 'disconnected' as const,
    };
  const bushaConnected = bushaAccount.status === 'connected';
  const bushaExpired =
    bushaAccount.status === 'expired'
    || settings?.cryptoSession === 'expiredNeedsReconnect';
  const cryptoMode =
    bushaConnected
    || bushaExpired
    || settings?.assetClass === 'crypto'
    || settings?.cryptoSession === 'expiredNeedsReconnect';

  return (
    <div className="page">
      {bushaExpired ? (
        <div className="banner banner-warn" role="status" style={{ marginBottom: 16 }}>
          Busha session expired. Live crypto trading is paused — you are still in crypto mode (not NGX sandbox).
          Sign in via the Busha login window, or tap Reconnect below.
          <button
            type="button"
            className="btn btn-primary"
            style={{ marginLeft: 12 }}
            disabled={authBridgeBusy === 'busha'}
            onClick={() => void connectAuthBridge('busha')}
          >
            {authBridgeBusy === 'busha' ? 'Reconnecting…' : 'Reconnect Busha'}
          </button>
          <button
            type="button"
            className="btn"
            style={{ marginLeft: 8 }}
            disabled={authBridgeBusy === 'busha'}
            onClick={() => setConfirmKind('busha')}
          >
            Leave crypto
          </button>
        </div>
      ) : null}
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
            onClick={() => setConfirmKind('logout')}
          >
            {busy ? <IconSpinner /> : <IconLogout />}
            Log out
          </button>
        </div>
      </section>

      <section className="panel" aria-labelledby="broker-heading" style={{ marginTop: 20 }}>
        <h2 id="broker-heading">Live broker</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          {cryptoMode
            ? 'Busha is the live crypto venue in NGN. NGX brokers (Wealth / Bamboo) are ignored until you disconnect Busha. Pulse still unlocks the app but is not used for crypto prices.'
            : 'One NGX live broker at a time (Wealth or Bamboo). Pulse stays the market-data source. Connect Busha to switch the app into crypto trading mode. Without a connected live venue, Pulsar runs in sandbox mode.'}
        </p>
        <div className="broker-grid">
          <div role="radiogroup" aria-labelledby="broker-heading" className="broker-grid__ngx">
          {(brokers.length ? brokers : DEFAULT_BROKERS).map((b) => {
            const selected = !cryptoMode && selectedBroker === b.id;
            return (
              <button
                key={b.id}
                type="button"
                role="radio"
                aria-checked={selected}
                className={`broker-card${selected ? ' is-selected' : ''}`}
                disabled={wealthBusy || cryptoMode}
                title={
                  cryptoMode
                    ? 'Disconnect Busha to use NGX brokers'
                    : undefined
                }
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
          <button
            type="button"
            className={`broker-card${bushaConnected || bushaExpired ? ' is-selected' : ''}`}
            aria-pressed={bushaConnected || bushaExpired}
            title={
              bushaConnected
                ? 'Busha is the active live venue — see account details below'
                : bushaExpired
                  ? 'Reconnect Busha'
                  : 'Connect Busha'
            }
            disabled={authBridgeBusy === 'busha'}
            onClick={() => {
              if (bushaConnected) return;
              void connectAuthBridge('busha');
            }}
          >
            <img src={BROKER_LOGOS.busha} alt="" className="broker-card__logo" />
            <span className="broker-card__name">Busha</span>
            <span className="broker-card__meta">
              {authBridgeBusy === 'busha'
                ? 'Connecting'
                : bushaConnected
                  ? 'Connected'
                  : bushaExpired
                    ? 'Expired · Reconnect'
                    : 'Crypto · NGN'}
            </span>
          </button>
        </div>
      </section>

      {cryptoMode ? (
      <section className="panel" aria-labelledby="busha-heading" style={{ marginTop: 20 }}>
        <h2 id="busha-heading">Busha</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Cash and crypto lots come from Busha in NGN. Cycles place live transfers when live trading is on.
          When connecting, check <strong>Keep me logged in</strong> on Busha so Pulsar can renew your session silently.
          Disconnect to leave crypto mode and restore NGX brokers.
        </p>
        {bushaExpired ? (
          <div className="banner banner-warn" style={{ marginBottom: 12 }} role="status">
            Session expired. Sign in via the Busha login window, or tap Reconnect below.
          </div>
        ) : null}
        <div className="profile-card__badges" style={{ marginBottom: 12 }}>
          <span className={`status-pill ${bushaConnected ? 'status-pill--ok' : 'status-pill--warn'}`}>
            <span className={`live-dot ${bushaConnected ? '' : 'live-dot--off'}`} aria-hidden />
            {bushaConnected ? 'Connected' : 'Expired · reconnect'}
          </span>
          <span className="status-pill status-pill--muted">Crypto · NGN</span>
          {liveTradingEnabled ? (
            <span className="status-pill status-pill--ok">Live trading on</span>
          ) : (
            <span className="status-pill status-pill--muted">Live trading off</span>
          )}
        </div>
        <dl className="profile-meta">
          <div className="profile-meta__item">
            <dt>Account</dt>
            <dd>{bushaAccount.accountHint || '—'}</dd>
          </div>
          <div className="profile-meta__item">
            <dt>NGN cash</dt>
            <dd className="mono">{bushaCash != null ? formatNaira(bushaCash) : '—'}</dd>
          </div>
          <div className="profile-meta__item">
            <dt>Session expires</dt>
            <dd className="mono">
              {bushaAccount.expiresAt
                ? new Date(bushaAccount.expiresAt).toLocaleString(undefined, {
                    month: 'short',
                    day: 'numeric',
                    hour: '2-digit',
                    minute: '2-digit',
                  })
                : '—'}
            </dd>
          </div>
        </dl>
        <div className="btn-row" style={{ marginTop: 16 }}>
          {bushaExpired ? (
            <button
              type="button"
              className="btn btn-primary"
              disabled={authBridgeBusy === 'busha'}
              onClick={() => void connectAuthBridge('busha')}
            >
              {authBridgeBusy === 'busha' ? <IconSpinner /> : null}
              {authBridgeBusy === 'busha' ? 'Reconnecting…' : 'Reconnect Busha'}
            </button>
          ) : null}
          <button
            type="button"
            className="btn btn-secondary"
            disabled={authBridgeBusy === 'busha'}
            onClick={() => setConfirmKind('busha')}
          >
            {authBridgeBusy === 'busha' ? <IconSpinner /> : null}
            Disconnect Busha
          </button>
        </div>
      </section>
      ) : null}

      {!cryptoMode && (wealth?.connected || brokerIsConnected(selectedBroker, settings)) ? (
      <section className="panel" aria-labelledby="wealth-heading" style={{ marginTop: 20 }}>
        <h2 id="wealth-heading">{selectedBroker === 'bamboo' ? 'Bamboo' : 'Coronation Wealth'}</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          {selectedBroker === 'bamboo'
            ? 'Cash and lots come from Bamboo. Approved signals place real NGX market orders when live trading is on.'
            : 'Cash and portfolio come from Wealth. Approved signals place real market orders when live trading is on.'}
        </p>
        {!wealth?.connected && brokerIsConnected(selectedBroker, settings) ? (
          <div className="banner banner-warn" style={{ marginBottom: 12 }} role="status">
            {wealth?.message || 'Broker session expired. Sign in again below to restore live mode.'}
          </div>
        ) : null}
        <div className="profile-card__badges" style={{ marginBottom: 12 }}>
          <span className={`status-pill ${wealth?.tradingMode === 'live' ? 'status-pill--ok' : 'status-pill--warn'}`}>
            <span className={`live-dot ${wealth?.tradingMode === 'live' ? '' : 'live-dot--off'}`} aria-hidden />
            {wealth?.tradingMode === 'live' ? 'Live trader' : 'Sandbox (not verified)'}
          </span>
          {wealth?.tradingProfile ? (
            <span className="status-pill status-pill--muted">
              Trading profile: {wealth.tradingProfile}
            </span>
          ) : null}
        </div>
        <dl className="profile-meta">
          <div className="profile-meta__item">
            <dt>Account</dt>
            <dd>{wealth?.displayName || wealth?.email || settings?.bambooPhone || settings?.wealthEmail || '—'}</dd>
          </div>
          <div className="profile-meta__item">
            <dt>Brokerage balance</dt>
            <dd className="mono">
              {wealth?.brokerageBalance != null ? formatNaira(wealth.brokerageBalance) : '—'}
            </dd>
          </div>
        </dl>
        {wealth && !wealth.ok && wealth.connected && (
          <div className="banner banner-bad" style={{ marginTop: 12 }} role="status">
            {wealth.message}
          </div>
        )}
        {wealth?.ok && !wealth.tradingVerified && (
          <div className="banner banner-warn" style={{ marginTop: 12 }} role="status">
            {wealth.message}
          </div>
        )}
        {!wealth?.connected && brokerIsConnected(selectedBroker, settings) ? (
          <div style={{ marginTop: 16 }}>
            <button
              type="button"
              className="btn btn-primary"
              disabled={wealthBusy}
              onClick={() => setBrokerModalOpen(true)}
            >
              Reconnect {selectedBroker === 'bamboo' ? 'Bamboo' : 'Wealth'}
            </button>
          </div>
        ) : null}
        <div style={{ marginTop: 16 }}>
          <button
            type="button"
            className="btn btn-secondary"
            disabled={wealthBusy}
            onClick={() => setConfirmKind(selectedBroker === 'bamboo' ? 'bamboo' : 'wealth')}
          >
            {wealthBusy ? <IconSpinner /> : null}
            Disconnect {selectedBroker === 'bamboo' ? 'Bamboo' : 'Wealth'}
          </button>
        </div>
      </section>
      ) : null}

      <section className="panel" aria-labelledby="strategy-heading">
        <h2 id="strategy-heading">Strategy</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          {wealth?.tradingMode === 'live'
            ? cryptoMode
              ? 'Risk and sizing for live Busha crypto orders in NGN. The universe is NGN pairs from Busha, not NGX stocks.'
              : `Risk and sizing for live ${selectedBroker === 'bamboo' ? 'Bamboo' : 'Wealth'} orders. The trading universe is all active NGX instruments from Pulse.`
            : cryptoMode
              ? 'Connect Busha and enable live trading to execute crypto orders. Disconnect Busha to return to NGX stocks.'
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
            label="Cycle cash budget"
            hint="Share of available cash reserved for buys in a single cycle. 100% uses all spendable cash (still subject to the broker minimum, max position, fees, and whole shares). That budget is split across approved buys by model confidence."
            value={strategyDraft.cycleBudgetPct}
            min={5}
            max={100}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, cycleBudgetPct: v })}
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
            hint="If today’s equity (session-open vs now, or vs prior close) drops this far, new buys are blocked (`BLOCKED_DRAWDOWN`). 100% turns that BUY brake off. Protective sells still run. Flatten-on-drawdown is a separate, default-off arm below."
            value={strategyDraft.maxDailyDrawdownPct}
            min={1}
            max={100}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, maxDailyDrawdownPct: v })}
          />
          <ParamSlider
            id={stopLossId}
            label="Stop loss"
            hint="Sell an open position when last price is this far below average cost. Polled about every 30s during market hours while this app stays open. Quitting Pulsar stops the risk monitor — stop-loss, take-profit, and time-stop will not fill until you reopen."
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
            hint="Sell an open position when last price is this far above average cost. Same in-app risk monitor as stop-loss: it does not run after you quit."
            value={strategyDraft.takeProfitPct}
            min={2}
            max={40}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, takeProfitPct: v })}
          />
          <ParamSlider
            id={timeStopId}
            label="Time-stop"
            hint="If a holding is still in-band (neither SL nor TP) after this many hours, sell the lot to recycle cash under the broker minimum. 0 hours turns time-stop off. Does not flatten on drawdown."
            value={strategyDraft.timeStopHours}
            min={0}
            max={168}
            format={(v) => (v === 0 ? 'Off' : `${v} h`)}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, timeStopHours: v })}
          />
          <ParamSlider
            id={partialTpId}
            label="Partial take-profit"
            hint="Share of the lot to sell when take-profit hits. 100% exits the full position; 50% scales out half and leaves the rest running."
            value={strategyDraft.partialTpPct}
            min={10}
            max={100}
            step={5}
            format={(v) => `${v}%`}
            disabled={strategyBusy}
            onChange={(v) => setStrategyDraft({ ...strategyDraft, partialTpPct: v })}
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
          Pulsar is not a headless daemon. Closing this window stops cycle scheduling and the risk monitor
          (stop-loss / take-profit / time-stop). Auto-cycle off still submits those protective sells while
          the app remains open and a live broker is connected; it only stops timed ingest → signal cycles.
        </p>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label" htmlFor={soundsId}>Notification sounds</label>
            <p className="toggle-row__hint">
              Play a short cue when cycles finish, risk exits fire, errors occur, or Busha needs reconnect.
              Progress chatter (Saving…) stays silent.
            </p>
          </div>
          <button
            id={soundsId}
            type="button"
            role="switch"
            aria-checked={notificationSounds}
            className={`toggle ${notificationSounds ? 'is-on' : ''}`}
            onClick={() => {
              const next = !notificationSounds;
              setNotificationSounds(next);
              setNotificationSoundsEnabled(next);
              if (next) playNotificationSound('success');
            }}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label" htmlFor={autoCycleId}>Run cycles automatically</label>
            <p className="toggle-row__hint">
              When on, the app schedules full trading cycles during NGX hours. When off, cycles only run from Home — protective exits (stop-loss / take-profit / time-stop) still submit while a live broker is connected and the app is open.
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

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label">Enable live trading</label>
            <p className="toggle-row__hint">
              When on, Pulsar submits real broker orders for discretionary trading cycles. Stop-loss, take-profit, and time-stop always submit while a live broker is connected and the venue is open — this toggle does not gate those protective sells. Prefer Halt new buys if you want to freeze entries only.
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
            <label className="toggle-row__label" htmlFor={haltBuysId}>Halt new buys (keep exits)</label>
            <p className="toggle-row__hint">
              Protective-only: block new BUY capacity and live BUY submits, but still execute stop-loss, take-profit, and time-stop sells.
            </p>
          </div>
          <button
            id={haltBuysId}
            type="button"
            role="switch"
            aria-checked={haltNewBuys}
            className={`toggle ${haltNewBuys ? 'is-on' : ''}`}
            disabled={cycleBusy}
            onClick={() => setHaltNewBuys((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label" htmlFor={flattenId}>Arm flatten on drawdown</label>
            <p className="toggle-row__hint">
              Default off. When armed, a session drawdown at the strategy cap queues a full SELL of every lot (audited in memory). This is not the same as the BUY-only drawdown brake, and it will not arm itself from PnL.
            </p>
          </div>
          <button
            id={flattenId}
            type="button"
            role="switch"
            aria-checked={flattenArmed}
            className={`toggle ${flattenArmed ? 'is-on' : ''}`}
            disabled={cycleBusy}
            onClick={() => setFlattenArmed((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <label className="toggle-row" style={{ alignItems: 'flex-start', cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={appDownAck}
            onChange={(e) => {
              setAppDownAck(e.target.checked);
              if (!e.target.checked) setLaunchAtLogin(false);
            }}
            style={{ marginTop: 4 }}
          />
          <span className="toggle-row__hint" style={{ margin: 0 }}>
            I understand that quitting Pulsar stops stop-loss polling. Launch at login only reopens the app; it is not a watchdog.
          </span>
        </label>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label" htmlFor={launchId}>Launch at login (macOS)</label>
            <p className="toggle-row__hint">
              Optional. Gated until you acknowledge the app-down warning above. Packaged macOS builds register a login item; unpackaged debug runs store the flag only.
            </p>
          </div>
          <button
            id={launchId}
            type="button"
            role="switch"
            aria-checked={launchAtLogin}
            className={`toggle ${launchAtLogin ? 'is-on' : ''}`}
            disabled={cycleBusy || !appDownAck}
            onClick={() => setLaunchAtLogin((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <div className="param-slider-grid" style={{ marginTop: 20 }}>
          <ParamSlider
            id={cycleIntervalId}
            label="Cycle frequency"
            hint="Minutes between automatic cycles while the market is open (or in the post-close window). Hard auto-cycle failures retry after about 60s instead of burning this full interval."
            value={autoCycleMinutes}
            min={5}
            max={120}
            step={5}
            format={(v) => `${v} min`}
            disabled={cycleBusy || !autoCycleEnabled}
            onChange={setAutoCycleMinutes}
          />
          <ParamSlider
            id={maxActionsId}
            label="Max live actions"
            hint="Cap on live submits per cycle. At most 30% of this budget may be spent retrying 24h-old unexecuted orders so new signals and protective sells are not starved."
            value={maxLiveActions}
            min={1}
            max={40}
            format={(v) => `${v}`}
            disabled={cycleBusy}
            onChange={setMaxLiveActions}
          />
          <ParamSlider
            id={maxNotionalId}
            label="Max live notional"
            hint="Per-order notional cap in naira for live submits. Bamboo still parks buys below ₦5,000."
            value={maxLiveNotional}
            min={5_000}
            max={5_000_000}
            step={5_000}
            format={(v) => formatNaira(v)}
            disabled={cycleBusy}
            onChange={setMaxLiveNotional}
          />
        </div>

        <div className="toggle-row">
          <div className="toggle-row__copy">
            <label className="toggle-row__label" htmlFor={retainLogsId}>Retain raw LLM logs</label>
            <p className="toggle-row__hint">
              Store cycle LLM transcripts in the local database for audits. Off by default.
            </p>
          </div>
          <button
            id={retainLogsId}
            type="button"
            role="switch"
            aria-checked={retainRawLlmLogs}
            className={`toggle ${retainRawLlmLogs ? 'is-on' : ''}`}
            disabled={cycleBusy}
            onClick={() => setRetainRawLlmLogs((v) => !v)}
          >
            <span className="toggle__thumb" />
          </button>
        </div>

        <div className="btn-row">
          <button type="button" className="btn btn-primary" disabled={cycleBusy} onClick={() => void saveAutoCycle()}>
            {cycleBusy && <IconSpinner />}
            {cycleBusy ? 'Saving…' : 'Save automation'}
          </button>
        </div>
      </section>

      <section className="panel" aria-labelledby="journal-heading">
        <h2 id="journal-heading">Confidence journal</h2>
        <p className="muted" style={{ marginTop: 0, fontSize: 13, lineHeight: 1.45 }}>
          Closed-lot hit rate and average PnL by signal confidence bucket. Filled from outcome labels on fill/close — empty until live or sandbox closes exist.
        </p>
        {journal.length === 0 ? (
          <p className="muted" style={{ fontSize: 13 }}>No closed lots labeled yet.</p>
        ) : (
          <dl className="summary-row" style={{ marginTop: 16 }}>
            {journal.map((row) => (
              <Fragment key={row.bucket}>
                <dt>{row.bucket}</dt>
                <dd>
                  n={row.n} hit={(row.hitRate * 100).toFixed(0)}%
                  {row.avgPnl != null ? ` avg PnL ₦${Math.round(row.avgPnl).toLocaleString('en-NG')}` : ''}
                </dd>
              </Fragment>
            ))}
          </dl>
        )}
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
          <dt>Temperature</dt>
          <dd>{formatLlmTemperatureLabel(llmStatus.temperature)}</dd>
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

            <LlmTemperatureControl
              useProviderDefault={draftUseProviderDefault}
              temperature={draftTemperature}
              disabled={modalBusy}
              onUseProviderDefaultChange={setDraftUseProviderDefault}
              onTemperatureChange={setDraftTemperature}
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
              placeholder={
                llmStatus?.configured
                  ? 'Leave blank to keep current key'
                  : 'Paste API key'
              }
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

      {confirmKind && confirmCopy ? (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) closeConfirmModal();
          }}
        >
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby={confirmTitleId}
          >
            <div className="modal__header">
              <div>
                <h2 id={confirmTitleId}>{confirmCopy.title}</h2>
                <p className="muted" style={{ margin: '6px 0 0', fontSize: 13, lineHeight: 1.45 }}>
                  {confirmCopy.body}
                </p>
              </div>
              <button
                type="button"
                className="modal__close"
                aria-label="Close"
                onClick={closeConfirmModal}
                disabled={confirmBusy}
              >
                ×
              </button>
            </div>
            <div className="btn-row">
              <button
                type="button"
                className="btn btn-ghost"
                disabled={confirmBusy}
                onClick={closeConfirmModal}
              >
                Cancel
              </button>
              <button
                type="button"
                className={confirmCopy.danger ? 'btn btn-danger' : 'btn btn-primary'}
                disabled={confirmBusy}
                onClick={() => void runConfirmedAction()}
              >
                {confirmBusy ? <IconSpinner /> : null}
                {confirmBusy ? 'Working…' : confirmCopy.confirmLabel}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
