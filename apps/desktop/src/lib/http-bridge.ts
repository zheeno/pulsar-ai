/**
 * Browser/dev fallback when Tauri is unavailable (no Xcode CLT).
 * Uses localStorage mock data — no Nest/Docker required.
 */

const SETTINGS_KEY = 'pulsar.browser.settings';
const SECRETS_KEY = 'pulsar.browser.secrets';
const STORE_KEY = 'pulsar.browser.store';

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
      };
      delete store.strategy.allowed_symbols;
      saveStore(store);
      return store.strategy as T;
    }

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
