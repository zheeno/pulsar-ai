/**
 * Browser/dev fallback when Tauri is unavailable (no Xcode CLT).
 * Uses localStorage mock data — no Nest/Docker required.
 */

const SETTINGS_KEY = 'pulsar.browser.settings';
const SECRETS_KEY = 'pulsar.browser.secrets';
const STORE_KEY = 'pulsar.browser.store';

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
  performance: { snapshot_date: string; total_equity: number; pnl_daily: number }[];
  strategy: Record<string, unknown>;
  backtests: Record<string, unknown>;
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
      { snapshot_date: '2026-08-01', total_equity: 10_000_000, pnl_daily: 0 },
      { snapshot_date: '2026-08-05', total_equity: 10_150_000, pnl_daily: 50_000 },
      { snapshot_date: '2026-08-10', total_equity: 10_339_500, pnl_daily: 40_000 },
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
      is_active: true,
    },
    backtests: {},
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

function loadSecrets(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(SECRETS_KEY) || '{}');
  } catch {
    return {};
  }
}

function saveSecrets(partial: Record<string, string | undefined>) {
  const current = loadSecrets();
  for (const [k, v] of Object.entries(partial)) {
    if (v === undefined || v === '') delete current[k];
    else current[k] = v;
  }
  localStorage.setItem(SECRETS_KEY, JSON.stringify(current));
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
      saveSecrets({
        pulsePassword: payload.pulsePassword,
        pulseApiKey: payload.pulseApiKey,
        llmApiKey: payload.llmApiKey,
      });
      return undefined as T;
    }

    case 'logout': {
      const next = {
        ...loadSettings(),
        pulseEmail: undefined,
        pulseConfigured: false,
        llmConfigured: false,
        onboardingComplete: false,
      };
      delete next.pulseEmail;
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
        tokenPreview: null,
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
      const market_value = store.positions.reduce((s, p) => s + p.market_value, 0);
      return {
        portfolio: store.portfolio,
        positions: store.positions,
        total_equity: store.portfolio.cash_balance + market_value,
        market_value,
        pnl_today: store.performance.at(-1)?.pnl_daily ?? 0,
      } as T;
    }

    case 'portfolio_performance':
      return loadStore().performance as T;

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
      saveStore(store);
      return { signals: 1, executed: 0, signalIds: [id], count: 1, warnings: ['Browser mock mode'] } as T;
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
          maxDailyTrades?: number;
          stopLossPct?: number;
          takeProfitPct?: number | null;
          minConfidenceToTrade?: number;
          maxDailyDrawdownPct?: number;
          positionSizePct?: number;
        };
      };
      const s = payload.strategy || {};
      const store = loadStore();
      store.strategy = {
        ...store.strategy,
        max_position_pct: s.maxPositionPct ?? store.strategy.max_position_pct,
        max_daily_trades: s.maxDailyTrades ?? store.strategy.max_daily_trades,
        stop_loss_pct: s.stopLossPct ?? store.strategy.stop_loss_pct,
        take_profit_pct: s.takeProfitPct ?? store.strategy.take_profit_pct ?? 0.1,
        min_confidence_to_trade: s.minConfidenceToTrade ?? store.strategy.min_confidence_to_trade,
        max_daily_drawdown_pct: s.maxDailyDrawdownPct ?? store.strategy.max_daily_drawdown_pct,
        position_size_pct: s.positionSizePct ?? store.strategy.position_size_pct,
      };
      delete store.strategy.allowed_symbols;
      saveStore(store);
      return store.strategy as T;
    }

    case 'start_backtest': {
      const runId = `bt-${Date.now()}`;
      const store = loadStore();
      store.backtests[runId] = {
        id: runId,
        status: 'completed',
        results: {
          final_equity: 10_500_000,
          total_trades: 12,
          win_rate: 0.58,
          note: 'Browser mock backtest',
        },
      };
      saveStore(store);
      return runId as T;
    }

    case 'get_backtest': {
      const runId = String(args?.runId || '');
      const run = loadStore().backtests[runId];
      if (!run) throw new Error('Backtest not found');
      return run as T;
    }

    case 'export_database':
      return 'browser-mock (localStorage)' as T;

    case 'app_data_dir':
      return 'browser-mock' as T;

    default:
      throw new Error(`Unknown command in browser mode: ${command}`);
  }
}
