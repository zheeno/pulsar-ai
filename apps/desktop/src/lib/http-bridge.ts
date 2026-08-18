/**
 * Browser/dev fallback when Tauri is unavailable (no Xcode CLT).
 * Uses localStorage mock data — no Nest/Docker required.
 */

const SETTINGS_KEY = 'pulsar.browser.settings';
const SECRETS_KEY = 'pulsar.browser.secrets';
const STORE_KEY = 'pulsar.browser.store';
const COACH_KEY = 'pulsar.browser.coach';

try {
  localStorage.removeItem(SECRETS_KEY);
} catch {
  /* ignore */
}

type Settings = Record<string, unknown>;

interface MockStore {
  portfolio: {
    id: string;
    name: string;
    starting_capital: number;
    cash_balance: number;
    strategy_param_set_id: string;
  };
  positions: {
    symbol: string;
    quantity: number;
    avg_cost: number;
    current_price: number;
    market_value: number;
  }[];
  signals: Record<string, unknown>[];
  trades: Record<string, unknown>[];
  performance: { recorded_at: string; snapshot_date: string; total_equity: number; pnl_daily: number }[];
  strategy: Record<string, unknown>;
  memories: {
    id: string;
    kind: string;
    symbol?: string | null;
    text: string;
    source: string;
    createdAt: string;
  }[];
}

function defaultSettings(): Settings {
  return {
    pulseBaseUrl: 'https://ngxpulse.ng/api',
    llmProvider: 'openai',
    llmModel: 'gpt-4o-mini',
    pulseConfigured: false,
    llmConfigured: false,
    onboardingComplete: false,
    defaultStartingCapital: 10_000_000,
    simulatedSlippageBps: 10,
    simulatedFeePct: 0.0015,
    autoCycleEnabled: false,
    autoCycleIntervalMinutes: 30,
    selectedBroker: 'wealth',
    wealthEmail: undefined,
    wealthConnected: false,
    bambooPhone: undefined,
    bambooConnected: false,
    liveTradingEnabled: false,
    maxLiveNotional: 500_000,
    maxLiveActions: 10,
    retainRawLlmLogs: false,
    haltNewBuys: false,
    flattenOnDrawdownArmed: false,
    launchAtLogin: false,
  };
}

function defaultStore(): MockStore {
  const strategyId = 'mock-strategy-1';
  const portfolioId = 'mock-portfolio-1';
  return {
    portfolio: {
      id: portfolioId,
      name: 'default-sandbox',
      starting_capital: 10_000_000,
      cash_balance: 8_500_000,
      strategy_param_set_id: strategyId,
    },
    positions: [
      { symbol: 'GTCO', quantity: 10000, avg_cost: 45, current_price: 46.2, market_value: 462_000 },
      { symbol: 'MTNN', quantity: 2000, avg_cost: 220, current_price: 225, market_value: 450_000 },
      { symbol: 'DANGCEM', quantity: 1500, avg_cost: 280, current_price: 285, market_value: 427_500 },
    ],
    signals: [
      {
        id: 'sig-1',
        symbol: 'GTCO',
        generated_at: new Date().toISOString(),
        action: 'BUY',
        confidence: 0.72,
        rationale: 'Mock signal — browser mode',
        model_name: 'mock-llm',
        executed: false,
        risk_policy_result: 'APPROVED',
      },
    ],
    trades: [],
    performance: [
      { recorded_at: '2026-08-01T00:00:00Z', snapshot_date: '2026-08-01', total_equity: 10_000_000, pnl_daily: 0 },
      { recorded_at: '2026-08-05T00:00:00Z', snapshot_date: '2026-08-05', total_equity: 10_150_000, pnl_daily: 50_000 },
      { recorded_at: '2026-08-10T00:00:00Z', snapshot_date: '2026-08-10', total_equity: 10_339_500, pnl_daily: 40_000 },
    ],
    strategy: {
      id: strategyId,
      name: 'default',
      max_position_pct: 0.1,
      max_daily_trades: 5,
      stop_loss_pct: 0.05,
      take_profit_pct: 0.1,
      min_confidence_to_trade: 0.65,
      max_daily_drawdown_pct: 0.03,
      position_size_pct: 0.05,
      cycle_budget_pct: 0.2,
      time_stop_hours: 24,
      partial_tp_fraction: 1,
      is_active: true,
    },
    memories: [],
  };
}

function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    return raw ? { ...defaultSettings(), ...JSON.parse(raw) } : defaultSettings();
  } catch {
    return defaultSettings();
  }
}

function saveSettingsLocal(settings: Settings) {
  localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
}

function selectedBrokerId(): string {
  return String(loadSettings().selectedBroker || 'wealth');
}

function mockLiveConnected(): boolean {
  const s = loadSettings();
  const id = selectedBrokerId();
  if (id === 'bamboo') return Boolean(s.bambooConnected);
  return id === 'wealth' && Boolean(s.wealthConnected);
}

function mockBrokerMeta() {
  const id = selectedBrokerId();
  return {
    brokerId: id,
    brokerName: id === 'bamboo' ? 'Bamboo' : 'Wealth',
  };
}

const COACH_BOUNDS: Record<string, [number, number]> = {
  max_position_pct: [0.01, 0.5],
  cycle_budget_pct: [0.05, 1],
  min_confidence_to_trade: [0.4, 0.95],
  max_daily_drawdown_pct: [0.01, 1],
  stop_loss_pct: [0.01, 0.25],
  take_profit_pct: [0.02, 0.4],
};

const COACH_LABELS: Record<string, string> = {
  max_position_pct: 'Max position',
  cycle_budget_pct: 'Cycle cash budget',
  min_confidence_to_trade: 'Min confidence',
  max_daily_drawdown_pct: 'Max daily drawdown',
  stop_loss_pct: 'Stop loss',
  take_profit_pct: 'Take profit',
};

const COACH_DEFAULTS: Record<string, number> = {
  max_position_pct: 0.1,
  cycle_budget_pct: 0.2,
  min_confidence_to_trade: 0.65,
  max_daily_drawdown_pct: 0.03,
  stop_loss_pct: 0.05,
  take_profit_pct: 0.1,
};

function clampCoach(field: string, value: number): number {
  const [min, max] = COACH_BOUNDS[field] || [0, 1];
  return Math.min(max, Math.max(min, value));
}

function currentCoachParams(store: MockStore): Record<string, number> {
  return {
    max_position_pct: Number(store.strategy.max_position_pct ?? 0.1),
    cycle_budget_pct: Number(store.strategy.cycle_budget_pct ?? 0.2),
    min_confidence_to_trade: Number(store.strategy.min_confidence_to_trade ?? 0.65),
    max_daily_drawdown_pct: Number(store.strategy.max_daily_drawdown_pct ?? 0.03),
    stop_loss_pct: Number(store.strategy.stop_loss_pct ?? 0.05),
    take_profit_pct: Number(store.strategy.take_profit_pct ?? 0.1),
  };
}

function coachDiff(
  current: Record<string, number>,
  patch: Record<string, number>,
  rationale: Record<string, string>,
) {
  return Object.entries(patch)
    .filter(([, v]) => Number.isFinite(v))
    .filter(([field, v]) => Math.abs(v - (current[field] ?? v)) >= 1e-9)
    .map(([field, proposed]) => ({
      field,
      label: COACH_LABELS[field] || field,
      current: current[field],
      proposed,
      currentPct: Math.round((current[field] ?? 0) * 100),
      proposedPct: Math.round(proposed * 100),
      rationale: rationale[field] || '',
    }));
}

function mockCoachWarnings(cash: number, patch: Record<string, number>): string[] {
  const warnings: string[] = [];
  const keys = Object.keys(patch);
  if (!keys.length) return warnings;
  const buyRelated = ['max_position_pct', 'cycle_budget_pct', 'min_confidence_to_trade'].some(
    (k) => patch[k] != null,
  );
  if (buyRelated && cash + 1e-9 < 5000) {
    warnings.push('Cash is below ₦5000; Bamboo cannot place a BUY.');
  }
  if (Math.abs((patch.cycle_budget_pct ?? 0) - 1) < 1e-9 && patch.cycle_budget_pct != null) {
    warnings.push('Cycle budget 100% means no extra cash cap beyond spendable cash.');
  }
  if (Math.abs((patch.max_daily_drawdown_pct ?? 0) - 1) < 1e-9 && patch.max_daily_drawdown_pct != null) {
    warnings.push('Drawdown 100% means session-loss halt is off.');
  }
  return warnings;
}

function pct(ratio: number) {
  return `${Math.round(ratio * 100)}%`;
}

function mockStrategyCoachPropose(message: string, store: MockStore) {
  const current = currentCoachParams(store);
  const cash = Number(store.portfolio.cash_balance ?? 0);
  const text = message.toLowerCase();
  const tighten = /fear|loss|losses|scared|tight|conservative|risk off/.test(text);
  const deploy = /idle|deploy|turnover|more trades|aggressive|put cash/.test(text);
  const lock = /lock|take profit|\btp\b|gains|profit target/.test(text);
  const reset = /reset|default|balanced|start over/.test(text);
  const wantsChange = /change|adjust|update|set |make |tighten|loosen|raise|lower|increase|decrease/.test(text);
  const decide = /you decide|your call|you should decide|appropriate response|you choose|you pick|don't have a strategy|dont have a strategy/.test(text);
  const conversational =
    /^(hi|hello|hey|yo)\b/.test(text.trim()) ||
    /what do you think|how is|review|current strategy|look at my|assess|why|explain/.test(text);

  if (conversational && !decide && !tighten && !deploy && !lock && !reset) {
    return {
      needMoreContext: false,
      clarifyingQuestions: [],
      summary: `Your sliders are max position ${pct(current.max_position_pct)}, cycle cash ${pct(current.cycle_budget_pct)}, min confidence ${pct(current.min_confidence_to_trade)}, stop ${pct(current.stop_loss_pct)}, take-profit ${pct(current.take_profit_pct)}. Cash on this book is ₦${Math.round(cash).toLocaleString('en-NG')}. Nothing has been saved — say if you want to tighten risk, deploy cash, or lock gains.`,
      patch: {},
      rationale: {},
      warnings: [],
      current,
      diff: [],
    };
  }

  if (decide || /stop loss|take profit|tolerate a \d/.test(text)) {
    const patch: Record<string, number> = {};
    const rationale: Record<string, string> = {};
    const ddMatch = text.match(/(\d+(?:\.\d+)?)\s*%/);
    if (ddMatch && /drop|drawdown|tolerate/.test(text)) {
      patch.max_daily_drawdown_pct = clampCoach('max_daily_drawdown_pct', Number(ddMatch[1]) / 100);
      rationale.max_daily_drawdown_pct = 'Matches the drop you said you can tolerate.';
    }
    const sl = clampCoach('stop_loss_pct', Math.min(0.12, (patch.max_daily_drawdown_pct ?? current.max_daily_drawdown_pct) * 0.65));
    const tp = clampCoach('take_profit_pct', 0.18);
    if (Math.abs(sl - current.stop_loss_pct) >= 1e-9) {
      patch.stop_loss_pct = sl;
      rationale.stop_loss_pct = 'Stop inside your drawdown band so one name cannot exhaust the daily cap.';
    }
    if (Math.abs(tp - current.take_profit_pct) >= 1e-9) {
      patch.take_profit_pct = tp;
      rationale.take_profit_pct = 'Position take-profit — not a 20% daily account target, which is not a slider.';
    }
    return {
      needMoreContext: false,
      clarifyingQuestions: [],
      summary: 'I decided from what you already said. Confirm the diff to save — nothing has been applied yet.',
      patch,
      rationale,
      warnings: mockCoachWarnings(cash, patch),
      current,
      diff: coachDiff(current, patch, rationale),
    };
  }

  if (!tighten && !deploy && !lock && !reset) {
    return {
      needMoreContext: wantsChange,
      clarifyingQuestions: wantsChange
        ? [
            'Do you want to take less risk, put idle cash to work, or lock gains sooner?',
            'Is this for the current account size, or are you resetting to balanced defaults?',
          ]
        : [],
      summary: wantsChange
        ? 'I need a clearer goal before proposing slider changes. Nothing has been saved.'
        : `Happy to talk through the book. Cash is ₦${Math.round(cash).toLocaleString('en-NG')}; cycle budget is ${pct(current.cycle_budget_pct)}. Ask a question or name a change.`,
      patch: {},
      rationale: {},
      warnings: [],
      current,
      diff: [],
    };
  }

  const patch: Record<string, number> = {};
  const rationale: Record<string, string> = {};
  let summary = '';

  if (reset) {
    for (const [field, value] of Object.entries(COACH_DEFAULTS)) {
      const next = clampCoach(field, value);
      if (Math.abs(next - current[field]) >= 1e-9) {
        patch[field] = next;
        rationale[field] = 'Balanced default used by Settings for a typical ₦ account.';
      }
    }
    summary = `Reset toward balanced defaults using the current cash of ₦${Math.round(cash).toLocaleString('en-NG')}. Confirm to save; Settings sliders will update.`;
  } else if (tighten) {
    patch.max_position_pct = clampCoach('max_position_pct', current.max_position_pct * 0.8);
    patch.cycle_budget_pct = clampCoach('cycle_budget_pct', current.cycle_budget_pct * 0.75);
    patch.min_confidence_to_trade = clampCoach('min_confidence_to_trade', current.min_confidence_to_trade + 0.05);
    rationale.max_position_pct = 'Smaller single-name weight after losses.';
    rationale.cycle_budget_pct = 'Spend less cash per cycle until confidence recovers.';
    rationale.min_confidence_to_trade = 'Block weaker signals while risk is elevated.';
    summary = 'Tighten risk from the current sliders. Confirm to save — nothing is live until you apply.';
  } else if (deploy) {
    patch.cycle_budget_pct = clampCoach('cycle_budget_pct', Math.min(1, current.cycle_budget_pct + 0.15));
    rationale.cycle_budget_pct = 'Raise the per-cycle share of current spendable cash. Fills already reduced the wallet.';
    summary = `Deploy more of the current ₦${Math.round(cash).toLocaleString('en-NG')} cash each cycle. Confirm to save.`;
  } else if (lock) {
    patch.take_profit_pct = clampCoach('take_profit_pct', current.take_profit_pct * 0.7);
    rationale.take_profit_pct = 'Lower the take-profit so open winners are realized sooner.';
    summary = 'Lower take-profit from the current slider to lock gains sooner. Confirm to save.';
  }

  for (const field of Object.keys(patch)) {
    patch[field] = clampCoach(field, patch[field]);
    if (Math.abs(patch[field] - current[field]) < 1e-9) delete patch[field];
  }

  return {
    needMoreContext: false,
    clarifyingQuestions: [],
    summary,
    patch,
    rationale,
    warnings: mockCoachWarnings(cash, patch),
    current,
    diff: coachDiff(current, patch, rationale),
  };
}

function loadSecrets(): Record<string, string> {
  return {};
}

function saveSecrets(_partial: Record<string, string | undefined>) {
  try {
    localStorage.removeItem(SECRETS_KEY);
  } catch {
    /* ignore */
  }
}

function loadStore(): MockStore {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    return raw ? { ...defaultStore(), ...JSON.parse(raw) } : defaultStore();
  } catch {
    return defaultStore();
  }
}

function saveStore(store: MockStore) {
  localStorage.setItem(STORE_KEY, JSON.stringify(store));
}

type CoachStore = {
  sessions: { id: string; title: string; createdAt: string; updatedAt: string }[];
  transcripts: Record<string, Record<string, unknown>[]>;
  proposals: Record<string, Record<string, unknown>>;
};

function loadCoachStore(): CoachStore {
  try {
    const raw = localStorage.getItem(COACH_KEY);
    if (raw) return JSON.parse(raw) as CoachStore;
  } catch {
    /* ignore */
  }
  return { sessions: [], transcripts: {}, proposals: {} };
}

function saveCoachStore(store: CoachStore) {
  localStorage.setItem(COACH_KEY, JSON.stringify(store));
}

export async function httpInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  switch (command) {
    case 'ping':
      return { message: 'pong', agent: { pong: true, mode: 'browser-mock' } } as T;

    case 'settings_get':
      return loadSettings() as T;

    case 'settings_set': {
      const payload = (args || {}) as {
        settings?: Settings;
        pulsePassword?: string;
        pulseApiKey?: string;
        llmApiKey?: string;
      };
      const current = loadSettings();
      const next = { ...current, ...(payload.settings || {}) };
      // Match Rust: only complete_onboarding may flip this to true.
      next.onboardingComplete = current.onboardingComplete;
      saveSettingsLocal(next);
      if (payload.pulsePassword || payload.pulseApiKey || payload.llmApiKey) {
        throw new Error('Credentials cannot be stored in browser mock mode — use the Tauri desktop app');
      }
      saveSecrets({});
      return undefined as T;
    }

    case 'logout': {
      const next = {
        ...loadSettings(),
        pulseEmail: undefined,
        pulseConfigured: false,
        llmConfigured: false,
        onboardingComplete: false,
        wealthEmail: undefined,
        wealthConnected: false,
        bambooPhone: undefined,
        bambooConnected: false,
      };
      delete next.pulseEmail;
      delete next.wealthEmail;
      delete next.bambooPhone;
      saveSettingsLocal(next);
      localStorage.removeItem(SECRETS_KEY);
      return undefined as T;
    }

    case 'test_pulse_login':
      return {
        ok: false,
        authMode: 'mock',
        supabaseHost: null,
        pulseBaseUrl: 'https://ngxpulse.ng/api',
        hasEmail: Boolean(loadSettings().pulseEmail),
        email: loadSettings().pulseEmail ?? null,
        hasPassword: Boolean(loadSecrets().pulsePassword),
        hasAnonKey: false,
        loginUrl: null,
        httpStatus: null,
        tokenExpiresAt: null,
        message: 'Browser mock — use Tauri desktop for real Pulse auth',
        logs: ['browser-mock: no network call'],
      } as T;

    case 'complete_onboarding':
      throw new Error(
        'complete_onboarding requires Tauri desktop (Pulse session + LLM verification)',
      );

    case 'test_llm':
      return { message: 'OK (browser mock — use Tauri for real LLM)' } as T;

    case 'llm_status': {
      const s = loadSettings();
      const key = loadSecrets().llmApiKey;
      const configured = Boolean(key);
      let maskedKey: string | null = null;
      if (key && key.length > 8) {
        maskedKey = `${key.slice(0, 3)}••••••••${key.slice(-4)}`;
      } else if (key) {
        maskedKey = '••••••••';
      }
      return {
        provider: s.llmProvider || 'openai',
        model: s.llmModel || 'gpt-4o-mini',
        baseUrl: s.llmBaseUrl ?? null,
        configured,
        maskedKey,
      } as T;
    }

    case 'portfolio_default': {
      const store = loadStore();
      const live = mockLiveConnected();
      const market_value = store.positions.reduce((s, p) => s + p.market_value, 0);
      return {
        portfolio: store.portfolio,
        positions: store.positions,
        total_equity: store.portfolio.cash_balance + market_value,
        market_value,
        pnl_today: store.performance.at(-1)?.pnl_daily ?? 0,
        tradingMode: live ? 'live' : 'sandbox',
        ...mockBrokerMeta(),
      } as T;
    }

    case 'portfolio_quotes': {
      const store = loadStore();
      const positions = store.positions.map((p) => {
        const last = p.current_price * (1 + (Math.random() - 0.5) * 0.002);
        return {
          ...p,
          current_price: last,
          market_value: p.quantity * last,
        };
      });
      saveStore({ ...store, positions });
      const market_value = positions.reduce((s, p) => s + p.market_value, 0);
      const unrealized_pnl = positions.reduce((s, p) => s + p.quantity * (p.current_price - p.avg_cost), 0);
      return {
        positions,
        total_equity: store.portfolio.cash_balance + market_value,
        market_value,
        pnl_today: store.performance.at(-1)?.pnl_daily ?? 0,
        unrealized_pnl,
        quotesAsOf: new Date().toISOString(),
        quotedSymbols: positions.map((p) => p.symbol),
        stale: false,
        tradingMode: mockLiveConnected() ? 'live' : 'sandbox',
        ...mockBrokerMeta(),
        portfolio: { id: store.portfolio.id, cash_balance: store.portfolio.cash_balance },
      } as T;
    }

    case 'portfolio_performance': {
      const venue = String(args?.venue || 'sandbox');
      if (venue === 'wealth') {
        return [] as T;
      }
      return loadStore().performance as T;
    }

    case 'usage_ngx_pulse':
      return { daily: 0, limit: null, remaining: null, authMode: 'mock' } as T;

    case 'market_status': {
      const now = new Date();
      const watHour = (now.getUTCHours() + 1) % 24;
      const isOpen = watHour >= 9 && watHour < 16;
      const isPostClose = watHour === 16 && now.getUTCMinutes() < 30;
      const phase = isOpen ? 'open' : isPostClose ? 'post_close' : 'closed';
      return {
        isOpen,
        isPostClose,
        isTradingDay: now.getUTCDay() !== 0 && now.getUTCDay() !== 6,
        phase,
        todayWat: now.toISOString().slice(0, 10),
        nowWat: `${String(watHour).padStart(2, '0')}:${String(now.getUTCMinutes()).padStart(2, '0')} WAT`,
        pulseStatus: isOpen ? 'Open' : 'Closed',
        pulseIsOpen: isOpen,
        appEnv: 'dev',
        marketHoursEnforced: false,
      } as T;
    }

    case 'cycle_status':
      return { running: false } as T;

    case 'symbol_detail': {
      const symbol = String(args?.symbol || 'GTCO').toUpperCase();
      const store = loadStore();
      const pos = store.positions.find((p) => p.symbol === symbol);
      return {
        symbol,
        name: pos ? `${symbol} Plc` : symbol,
        sector: 'Financial Services',
        found: true,
        latest: { date: '2026-08-12', price: pos?.current_price ?? 45, changePercent: 1.2 },
        prices: [
          { date: '2026-08-01', price: 44 },
          { date: '2026-08-05', price: 45 },
          { date: '2026-08-10', price: pos?.current_price ?? 46 },
        ],
        position: pos
          ? { quantity: pos.quantity, avgCost: pos.avg_cost }
          : null,
        signals: store.signals
          .filter((s) => s.symbol === symbol)
          .map((s) => ({
            id: s.id,
            generatedAt: s.generated_at,
            action: s.action,
            confidence: s.confidence,
            rationale: s.rationale,
            modelName: s.model_name,
            executed: s.executed,
            riskPolicyResult: s.risk_policy_result,
          })),
        trades: store.trades
          .filter((t: Record<string, unknown>) => t.symbol === symbol)
          .map((t: Record<string, unknown>) => ({
            id: t.id,
            side: t.side,
            quantity: t.quantity,
            fillPrice: t.fill_price,
            simulatedFee: t.simulated_fee,
            executedAt: t.executed_at,
            resultingCashBalance: t.resulting_cash_balance,
          })),
        pulseQuote: {
          price: pos?.current_price ?? 45,
          changePercent: 1.2,
          volume: 100000,
        },
        needsPulsePrices: false,
      } as T;
    }

    case 'symbol_detail_pulse': {
      const symbol = String(args?.symbol || 'GTCO').toUpperCase();
      return {
        symbol,
        prices: [
          { date: '2026-05-01', price: 40 },
          { date: '2026-06-01', price: 42 },
          { date: '2026-07-01', price: 44 },
          { date: '2026-08-01', price: 45 },
          { date: '2026-08-10', price: 46 },
        ],
        pulseQuote: { price: 46, changePercent: 2.2, volume: 120000, source: 'ngx_pulse' },
        error: null,
      } as T;
    }

    case 'cycle_run':
    case 'cycle_ingest':
    case 'generate_signals': {
      const store = loadStore();
      const id = `sig-${Date.now()}`;
      store.signals.unshift({
        id,
        symbol: 'ZENITHBANK',
        generated_at: new Date().toISOString(),
        action: 'HOLD',
        confidence: 0.55,
        rationale: 'Browser mock cycle — no live ingest/LLM',
        model_name: 'mock-llm',
        executed: false,
        risk_policy_result: 'BLOCKED_CONFIDENCE',
      });
      if (command === 'cycle_run') {
        const now = new Date();
        const today = now.toISOString().slice(0, 10);
        const last = store.performance.at(-1);
        const equity = last?.total_equity ?? 10_000_000;
        const priorClose = [...store.performance]
          .reverse()
          .find((p) => p.snapshot_date < today);
        store.performance.push({
          recorded_at: now.toISOString(),
          snapshot_date: today,
          total_equity: equity,
          pnl_daily: equity - (priorClose?.total_equity ?? equity),
        });
      }
      saveStore(store);
      return { signals: 1, executed: 0, signalIds: [id], count: 1, universeSize: 20, warnings: ['Browser mock mode'] } as T;
    }

    case 'list_signals': {
      const limit = Number(args?.limit ?? 50);
      return loadStore().signals.slice(0, limit) as T;
    }

    case 'list_trades': {
      const limit = Number(args?.limit ?? 50);
      return loadStore().trades.slice(0, limit) as T;
    }

    case 'get_strategy':
      return loadStore().strategy as T;

    case 'update_strategy': {
      const payload = (args || {}) as {
        strategy?: {
          maxPositionPct?: number;
          stopLossPct?: number;
          takeProfitPct?: number | null;
          minConfidenceToTrade?: number;
          maxDailyDrawdownPct?: number;
          cycleBudgetPct?: number;
          timeStopHours?: number;
          partialTpFraction?: number;
        };
      };
      const s = payload.strategy || {};
      const store = loadStore();
      store.strategy = {
        ...store.strategy,
        max_position_pct: s.maxPositionPct ?? store.strategy.max_position_pct,
        stop_loss_pct: s.stopLossPct ?? store.strategy.stop_loss_pct,
        take_profit_pct: s.takeProfitPct ?? store.strategy.take_profit_pct ?? 0.1,
        min_confidence_to_trade: s.minConfidenceToTrade ?? store.strategy.min_confidence_to_trade,
        max_daily_drawdown_pct: s.maxDailyDrawdownPct ?? store.strategy.max_daily_drawdown_pct,
        cycle_budget_pct: s.cycleBudgetPct ?? store.strategy.cycle_budget_pct ?? 0.2,
        time_stop_hours: s.timeStopHours ?? store.strategy.time_stop_hours ?? 24,
        partial_tp_fraction: s.partialTpFraction ?? store.strategy.partial_tp_fraction ?? 1,
      };
      delete store.strategy.allowed_symbols;
      saveStore(store);
      return store.strategy as T;
    }

    case 'coach_list_sessions': {
      return loadCoachStore().sessions as T;
    }

    case 'coach_new_session': {
      const store = loadCoachStore();
      const session = {
        id: `c-${Date.now()}`,
        title: 'New chat',
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
        messages: [] as Record<string, unknown>[],
      };
      store.sessions.unshift({ id: session.id, title: session.title, createdAt: session.createdAt, updatedAt: session.updatedAt });
      store.transcripts[session.id] = session.messages;
      saveCoachStore(store);
      return { ...session } as T;
    }

    case 'coach_get_session': {
      const store = loadCoachStore();
      const id = String(args?.id || store.sessions[0]?.id || '');
      if (!id) {
        const created = await httpInvoke('coach_new_session');
        return created as T;
      }
      const meta = store.sessions.find((s) => s.id === id) || store.sessions[0];
      return {
        ...meta,
        messages: store.transcripts[meta.id] || [],
      } as T;
    }

    case 'coach_delete_session': {
      const store = loadCoachStore();
      const id = String(args?.id || '');
      store.sessions = store.sessions.filter((s) => s.id !== id);
      delete store.transcripts[id];
      saveCoachStore(store);
      return undefined as T;
    }

    case 'coach_turn': {
      const message = String(args?.message || '').trim();
      if (!message) throw new Error('Message is required');
      let store = loadCoachStore();
      let sessionId = String(args?.sessionId || store.sessions[0]?.id || '');
      if (!sessionId) {
        const created = (await httpInvoke<{ id: string }>('coach_new_session')) as { id: string };
        sessionId = created.id;
        store = loadCoachStore();
      }
      const msgs = store.transcripts[sessionId] || [];
      msgs.push({ id: `u-${Date.now()}`, role: 'user', text: message, createdAt: new Date().toISOString() });
      const lower = message.toLowerCase();
      let summary = 'Browser mock Coach — use the Tauri app for live tools.';
      let toolTrace: { name: string; ok: boolean; summary: string }[] = [];
      let trade: Record<string, unknown> | null = null;
      let extra: Record<string, unknown> = {};
      const isMeta =
        /tell me about yourself|tell me about you|who are you|what can you do|how do you work|what are you|introduce yourself/.test(
          lower,
        );
      const isGreeting =
        /^(hi|hey|hello|yo|thanks|thank you|cheers|ty|bye|goodbye|later|see ya|see you)\b/.test(lower) &&
        lower.length <= 40;
      const isAdvisory =
        /\badvisable\b|\bshould i\b|\bthoughts on\b|\bworth buying\b/.test(lower);
      if (isMeta) {
        toolTrace = [];
        summary =
          "I'm Coach, Pulsar's NGX desk copilot. I can check the tape, a name's history, news when it's wired, help you tighten risk sliders, and propose trades. I won't silently place live orders, and I won't invent prices or headlines. What do you want to look at?";
      } else if (isGreeting) {
        toolTrace = [];
        summary = lower.includes('thank')
          ? 'Anytime. Ping me if you want the tape, a name, or a trade idea.'
          : 'Hey. Tape, a ticker, news, risk sliders, or a trade idea — your call.';
      } else if (isAdvisory || lower.includes('news')) {
        const news = lower.includes('news');
        toolTrace = news
          ? [{ name: 'get_news', ok: false, summary: 'unavailable' }]
          : [{ name: 'get_symbol_quote', ok: true, summary: 'GTCO @ 46.2' }];
        summary = news
          ? 'NGX news is not wired in this mock. No headlines were invented.'
          : 'GTCO last ₦46.20 as-of mock tape. Advice only — not an order.';
      } else if (lower.includes('moving') || lower.includes('quote')) {
        toolTrace = [{ name: 'list_universe_quotes', ok: true, summary: '2 quotes' }];
        summary = 'GTCO ₦46.20 (+1.2% as-of mock). MTNN ₦225.00. Figures are mock store data.';
      } else if (lower.includes('history') || lower.includes('doing')) {
        toolTrace = [{ name: 'get_price_history', ok: true, summary: 'GTCO' }];
        summary = 'GTCO recent closes come from the mock book, not a live Pulse call.';
      } else if (/\bbuy\b|\bsell\b/.test(lower)) {
        toolTrace = [{ name: 'propose_trade', ok: true, summary: 'proposal (not placed)' }];
        trade = {
          id: `p-${Date.now()}`,
          symbol: 'GTCO',
          side: lower.includes('sell') ? 'SELL' : 'BUY',
          quantity: 100,
          preview: { price: 46.2, estimatedCost: 4620, warnings: ['Browser mock — not sent to a broker'] },
          status: 'proposed',
        };
        store.proposals[String(trade.id)] = trade;
        summary = 'Proposed a GTCO order card. Confirm in the UI to submit — this chat did not place it.';
      } else {
        extra = mockStrategyCoachPropose(message, loadStore()) as Record<string, unknown>;
        summary = String(extra.summary || summary);
        toolTrace = [{ name: 'get_strategy_params', ok: true, summary: 'ok' }];
      }
      const payload = { toolTrace, trade, warnings: extra.warnings || [], diff: extra.diff || [], ...extra };
      msgs.push({
        id: `a-${Date.now()}`,
        role: 'assistant',
        text: summary,
        payload,
        createdAt: new Date().toISOString(),
      });
      store.transcripts[sessionId] = msgs;
      const meta = store.sessions.find((s) => s.id === sessionId);
      if (meta && msgs.filter((m) => m.role === 'user').length === 1) {
        meta.title = message.slice(0, 72);
        meta.updatedAt = new Date().toISOString();
      }
      saveCoachStore(store);
      return { summary, toolTrace, trade, sessionId, ...extra, warnings: extra.warnings || [], diff: extra.diff || [] } as T;
    }

    case 'coach_execute_trade': {
      const store = loadCoachStore();
      const id = String(args?.proposalId || '');
      const p = store.proposals[id];
      if (!p) throw new Error('Unknown trade proposal');
      if (p.status !== 'proposed') throw new Error('Proposal is not awaiting confirm');
      p.status = 'blocked';
      p.result = { ok: false, riskPolicyResult: 'BLOCKED_BROKER', error: 'Browser mock cannot place orders' };
      const sid = store.sessions[0]?.id;
      if (sid && store.transcripts[sid]) {
        store.transcripts[sid].push({
          id: `a-${Date.now()}`,
          role: 'assistant',
          text: 'Trade confirmed but not filled: BLOCKED_BROKER.',
          payload: { tradeResult: p.result, proposalId: id },
          createdAt: new Date().toISOString(),
        });
      }
      saveCoachStore(store);
      return p.result as T;
    }

    case 'coach_cancel_trade': {
      const store = loadCoachStore();
      const id = String(args?.proposalId || '');
      const p = store.proposals[id];
      if (!p) throw new Error('Unknown trade proposal');
      if (p.status !== 'proposed') throw new Error('Proposal is not awaiting confirm');
      p.status = 'cancelled';
      saveCoachStore(store);
      return undefined as T;
    }

    case 'strategy_coach_propose': {
      const message = String(args?.message || '').trim();
      if (!message) throw new Error('Message is required');
      return mockStrategyCoachPropose(message, loadStore()) as T;
    }

    case 'strategy_coach_apply': {
      const selected = (args?.selected || {}) as Record<string, number>;
      const keys = Object.keys(selected);
      if (!keys.length) throw new Error('Select at least one parameter to apply');
      const store = loadStore();
      const current = currentCoachParams(store);
      const next = { ...current };
      for (const [field, raw] of Object.entries(selected)) {
        if (!COACH_BOUNDS[field]) throw new Error(`Unknown strategy fields: ${field}`);
        next[field] = clampCoach(field, Number(raw));
      }
      if (!(next.cycle_budget_pct >= 0.05 && next.cycle_budget_pct <= 1)) {
        throw new Error('cycleBudgetPct must be between 0.05 and 1.0');
      }
      store.strategy = {
        ...store.strategy,
        max_position_pct: next.max_position_pct,
        cycle_budget_pct: next.cycle_budget_pct,
        min_confidence_to_trade: next.min_confidence_to_trade,
        max_daily_drawdown_pct: next.max_daily_drawdown_pct,
        stop_loss_pct: next.stop_loss_pct,
        take_profit_pct: next.take_profit_pct,
      };
      const rationale = (args?.rationale || {}) as Record<string, string>;
      const lines = ['Strategy coach applied:'];
      for (const field of keys) {
        lines.push(
          `- ${field}: ${current[field]?.toFixed(4)} → ${next[field]?.toFixed(4)}${
            rationale[field] ? ` (${rationale[field]})` : ''
          }`,
        );
      }
      const excerpt = String(args?.chatExcerpt || args?.chat_excerpt || '');
      if (excerpt) lines.push(`Chat: ${excerpt.slice(0, 400)}`);
      store.memories = [
        ...(store.memories || []),
        {
          id: `mem-${Date.now()}`,
          kind: 'freeform',
          text: lines.join('\n'),
          source: 'agent_upsert',
          createdAt: new Date().toISOString(),
        },
      ];
      saveStore(store);
      return {
        ok: true,
        strategy: store.strategy,
        message: 'Strategy parameters were saved. Settings sliders now show the new values.',
      } as T;
    }

    case 'confidence_journal':
      return [] as T;

    case 'list_cycle_audits':
      return [] as T;

    case 'memory_list': {
      return [...(loadStore().memories || [])].reverse() as T;
    }

    case 'memory_search': {
      const q = String(args?.query || '').toLowerCase();
      const symbol = args?.symbol ? String(args.symbol).toUpperCase() : '';
      return (loadStore().memories || []).filter((m) => {
        if (symbol && (m.symbol || '').toUpperCase() !== symbol) return false;
        if (!q) return true;
        return `${m.text} ${m.symbol || ''} ${m.kind}`.toLowerCase().includes(q);
      }) as T;
    }

    case 'memory_delete': {
      const id = String(args?.id || '');
      const store = loadStore();
      store.memories = (store.memories || []).filter((m) => m.id !== id);
      saveStore(store);
      return { ok: true } as T;
    }

    case 'export_database':
      return 'browser-mock (localStorage)' as T;

    case 'app_data_dir':
      return 'browser-mock' as T;

    case 'reset_local_data':
      localStorage.removeItem(SETTINGS_KEY);
      localStorage.removeItem(SECRETS_KEY);
      localStorage.removeItem(STORE_KEY);
      return { ok: true } as T;

    case 'wealth_login': {
      if (selectedBrokerId() !== 'wealth') {
        throw new Error('Select Coronation Wealth as the live broker before connecting.');
      }
      const email = String(args?.email || '');
      const password = String(args?.password || '');
      if (!email || !password) {
        return {
          ok: false,
          needs2fa: false,
          connected: false,
          message: 'Email and password required',
          email,
          baseUrl: 'https://wealthapp-api-stg-app.azurewebsites.net/v1',
        } as T;
      }
      if (password === '2fa') {
        return {
          ok: true,
          needs2fa: true,
          tempToken: 'mock-temp-token',
          connected: false,
          message: 'Two-factor authentication required.',
          email,
          baseUrl: 'https://wealthapp-api-stg-app.azurewebsites.net/v1',
        } as T;
      }
      const s = loadSettings();
      s.wealthEmail = email;
      s.wealthConnected = true;
      saveSettingsLocal(s);
      return {
        ok: true,
        needs2fa: false,
        connected: true,
        message: 'Wealth account connected.',
        email,
        baseUrl: 'https://wealthapp-api-stg-app.azurewebsites.net/v1',
      } as T;
    }

    case 'wealth_verify_2fa': {
      if (selectedBrokerId() !== 'wealth') {
        throw new Error('Select Coronation Wealth as the live broker before connecting.');
      }
      const email = String(args?.email || '');
      const s = loadSettings();
      s.wealthEmail = email;
      s.wealthConnected = true;
      saveSettingsLocal(s);
      return {
        ok: true,
        needs2fa: false,
        connected: true,
        message: 'Wealth account connected.',
        email,
        baseUrl: 'https://wealthapp-api-stg-app.azurewebsites.net/v1',
      } as T;
    }

    case 'wealth_profile': {
      const s = loadSettings();
      const connected = Boolean(s.wealthConnected);
      return {
        ok: connected,
        connected,
        email: s.wealthEmail ?? null,
        tradingProfile: connected ? 'verified' : null,
        tradingVerified: connected,
        tradingMode: mockLiveConnected() ? 'live' : 'sandbox',
        brokerageBalance: connected ? 250_000 : null,
        availableBalance: connected ? 200_000 : null,
        currentBalance: connected ? 250_000 : null,
        baseUrl: 'https://wealthapp-api-stg-app.azurewebsites.net/v1',
        message: connected
          ? 'Live trader mode — orders use Wealth brokerage balance.'
          : 'Wealth account not connected.',
        displayName: connected ? 'Mock Wealth User' : null,
      } as T;
    }

    case 'wealth_logout': {
      const s = loadSettings();
      s.wealthConnected = false;
      delete s.wealthEmail;
      saveSettingsLocal(s);
      return undefined as T;
    }

    case 'bamboo_login': {
      if (selectedBrokerId() !== 'bamboo') {
        throw new Error('Select Bamboo as the live broker before connecting.');
      }
      const phoneNumber = String(args?.phoneNumber || '');
      const password = String(args?.password || '');
      if (!phoneNumber || !password) {
        return {
          ok: false,
          connected: false,
          message: 'Phone number and password required',
          phone: phoneNumber,
          baseUrl: 'https://api.investbamboo.com',
        } as T;
      }
      const s = loadSettings();
      s.bambooPhone = phoneNumber;
      s.bambooConnected = true;
      saveSettingsLocal(s);
      return {
        ok: true,
        connected: true,
        message: 'Bamboo account connected.',
        phone: phoneNumber,
        baseUrl: 'https://api.investbamboo.com',
      } as T;
    }

    case 'bamboo_profile': {
      const s = loadSettings();
      const connected = Boolean(s.bambooConnected);
      return {
        ok: connected,
        connected,
        email: s.bambooPhone ?? null,
        tradingProfile: connected ? 'verified' : null,
        tradingVerified: connected,
        tradingMode: mockLiveConnected() ? 'live' : 'sandbox',
        brokerageBalance: connected ? 180_000 : null,
        availableBalance: connected ? 180_000 : null,
        currentBalance: connected ? 180_000 : null,
        baseUrl: 'https://api.investbamboo.com',
        message: connected
          ? 'Live trader mode — orders use Bamboo NGN cash.'
          : 'Bamboo account not connected.',
        displayName: connected ? 'Mock Bamboo User' : null,
      } as T;
    }

    case 'bamboo_logout': {
      const s = loadSettings();
      s.bambooConnected = false;
      delete s.bambooPhone;
      saveSettingsLocal(s);
      return undefined as T;
    }

    case 'broker_list': {
      const s = loadSettings();
      return [
        {
          id: 'wealth',
          name: 'Coronation Wealth',
          available: true,
          connected: Boolean(s.wealthConnected),
        },
        {
          id: 'bamboo',
          name: 'Bamboo',
          available: true,
          connected: Boolean(s.bambooConnected),
        },
      ] as T;
    }

    case 'set_selected_broker': {
      const brokerId = String(args?.brokerId || 'wealth') === 'bamboo' ? 'bamboo' : 'wealth';
      const s = loadSettings();
      s.selectedBroker = brokerId;
      saveSettingsLocal(s);
      return s as T;
    }

    default:
      throw new Error(`Unknown command in browser mode: ${command}`);
  }
}
