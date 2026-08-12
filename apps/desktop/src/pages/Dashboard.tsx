import { useEffect, useMemo, useState } from 'react';
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { IconShieldAlert, IconShieldCheck, IconSpinner } from '../components/Icons';
import { api, type PortfolioData } from '../lib/api';
import { useCycle } from '../lib/cycle';
import { useToast } from '../lib/toast';
import { Link } from 'react-router-dom';

type RawPortfolio = PortfolioData & {
  totalEquity?: number;
  marketValue?: number;
  pnlToday?: number;
  tradingVerified?: boolean;
  wealthStatus?: {
    connected?: boolean;
    tradingVerified?: boolean;
    message?: string;
    brokerageBalance?: number | null;
  } | null;
  wealthError?: string | null;
};

type Usage = {
  daily: number;
  limit: number | null;
  remaining: number | null;
  authMode?: string;
};

type MarketStatus = {
  isOpen: boolean;
  isPostClose: boolean;
  isTradingDay: boolean;
  phase: 'open' | 'post_close' | 'closed' | string;
  todayWat: string;
  nowWat: string;
  pulseStatus?: string | null;
  pulseIsOpen?: boolean | null;
  appEnv: string;
  marketHoursEnforced: boolean;
};

function normalizePortfolio(raw: RawPortfolio): PortfolioData {
  return {
    portfolio: raw.portfolio,
    positions: raw.positions ?? [],
    total_equity: Number(raw.total_equity ?? raw.totalEquity ?? 0),
    market_value: Number(raw.market_value ?? raw.marketValue ?? 0),
    pnl_today: Number(raw.pnl_today ?? raw.pnlToday ?? 0),
    tradingMode: (raw as { tradingMode?: string }).tradingMode || 'sandbox',
    tradingVerified: raw.tradingVerified ?? raw.wealthStatus?.tradingVerified,
    wealthStatus: raw.wealthStatus ?? null,
    wealthError: raw.wealthError ?? null,
  };
}

function marketPhaseLabel(phase: string): string {
  if (phase === 'open') return 'NGX Open';
  if (phase === 'post_close') return 'Post-close';
  return 'NGX Closed';
}

export default function DashboardPage() {
  const toast = useToast();
  const { running: cycleRunning } = useCycle();
  const [data, setData] = useState<PortfolioData | null>(null);
  const [performance, setPerformance] = useState<{ snapshot_date: string; total_equity: number }[]>([]);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [market, setMarket] = useState<MarketStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [cycleBusy, setCycleBusy] = useState(false);

  useEffect(() => {
    void loadData();
    const interval = setInterval(() => void loadData(), 30000);
    return () => clearInterval(interval);
  }, []);

  async function loadData() {
    try {
      const [portfolioRaw, usageData, marketData] = await Promise.all([
        api<RawPortfolio>('portfolio_default'),
        api<Usage>('usage_ngx_pulse'),
        api<MarketStatus>('market_status'),
      ]);
      const portfolio = normalizePortfolio(portfolioRaw);
      setData(portfolio);
      setUsage(usageData);
      setMarket(marketData);
      setError(null);
      const venue = portfolio.tradingMode === 'live' ? 'wealth' : 'sandbox';
      const perf = await api<{ snapshot_date: string; total_equity: number }[]>(
        'portfolio_performance',
        { venue, id: portfolio.portfolio?.id },
      );
      setPerformance(perf);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  async function runCycle() {
    if (cycleRunning || cycleBusy) return;
    setCycleBusy(true);
    try {
      await api('cycle_run');
      void loadData();
    } catch (e) {
      const msg = String(e);
      // Gate rejection has no cycle:complete event.
      if (msg.toLowerCase().includes('already running')) {
        toast.warning(msg, 'Cycle');
      }
      // Other failures emit cycle:complete (handled by CycleProvider).
    } finally {
      setCycleBusy(false);
    }
  }

  const cycleBlocked = cycleRunning || cycleBusy;

  const formatNaira = (n: number | null | undefined) => {
    const value = Number(n ?? 0);
    return `₦${(Number.isFinite(value) ? value : 0).toLocaleString('en-NG', { maximumFractionDigits: 0 })}`;
  };

  const status = useMemo(() => {
    if (error) {
      return {
        level: 'bad' as const,
        title: 'Attention needed',
        sub: error,
        live: false,
      };
    }
    if (loading && !data) {
      return {
        level: 'warn' as const,
        title: 'Checking systems…',
        sub: 'Loading portfolio and Pulse session status.',
        live: false,
      };
    }
    if (usage?.authMode === 'session' && data) {
      const live = data.tradingMode === 'live';
      return {
        level: 'ok' as const,
        title: 'Systems healthy',
        sub: live
          ? (data.tradingVerified
            ? 'NGX Pulse is active. Home shows your Wealth brokerage cash and holdings; cycles can place live market orders.'
            : (data.wealthStatus?.message
              || 'Wealth connected — Home shows brokerage cash and holdings. Complete trading verification in the Wealth app to enable live orders.'))
          : data.wealthError
            ? `Wealth connected but portfolio sync failed: ${data.wealthError}`
            : 'NGX Pulse session is active and your sandbox portfolio is ready. Connect a Wealth account in Settings to go live.',
        live: true,
      };
    }
    if (usage?.authMode && usage.authMode !== 'session') {
      return {
        level: 'warn' as const,
        title: 'Needs attention',
        sub: `Pulse is in ${usage.authMode} mode. Prefer a live session login for full market access.`,
        live: false,
      };
    }
    return {
      level: 'warn' as const,
      title: 'Needs attention',
      sub: 'Waiting for Pulse usage and portfolio data. Open Settings if this persists.',
      live: false,
    };
  }, [error, loading, data, usage]);

  const marketChipClass =
    market?.phase === 'open'
      ? 'status-pill--ok'
      : market?.phase === 'post_close'
        ? 'status-pill--warn'
        : 'status-pill--muted';

  return (
    <div className="page">
      <section className={`status-hero is-${status.level}`} aria-live="polite">
        <div className="status-hero__orb">
          {status.level === 'ok' ? <IconShieldCheck /> : <IconShieldAlert />}
        </div>
        <div className="status-hero__body">
          <div className="status-hero__meta">
            {status.live && <span className="live-dot" title="Live session" />}
            {status.live ? 'Protected · Live' : status.level === 'bad' ? 'Issue detected' : 'Review required'}
            {market ? (
              <>
                <span className="status-hero__meta-sep" aria-hidden>·</span>
                <span className={`status-pill ${marketChipClass}`} style={{ padding: '2px 8px', fontSize: '0.72rem' }}>
                  {market.phase === 'open' && <span className="live-dot" aria-hidden />}
                  {marketPhaseLabel(market.phase)}
                </span>
              </>
            ) : null}
            {cycleRunning ? (
              <>
                <span className="status-hero__meta-sep" aria-hidden>·</span>
                <span className="status-pill status-pill--warn" style={{ padding: '2px 8px', fontSize: '0.72rem' }}>
                  <IconSpinner size={12} />
                  Cycle running
                </span>
              </>
            ) : null}
            {data?.tradingMode ? (
              <>
                <span className="status-hero__meta-sep" aria-hidden>·</span>
                <span
                  className={`status-pill ${data.tradingMode === 'live' ? 'status-pill--ok' : 'status-pill--muted'}`}
                  style={{ padding: '2px 8px', fontSize: '0.72rem' }}
                >
                  {data.tradingMode === 'live'
                    ? (data.tradingVerified === false ? 'Wealth' : 'Live trader')
                    : 'Sandbox'}
                </span>
              </>
            ) : null}
          </div>
          <h1 className="status-hero__title">{status.title}</h1>
          <p className="status-hero__sub">{status.sub}</p>
          {market ? (
            <p className="status-hero__market muted">
              {market.nowWat}
              {market.pulseStatus ? ` · Pulse: ${market.pulseStatus}` : ''}
              {!market.marketHoursEnforced ? ' · Hours bypassed (dev)' : ''}
            </p>
          ) : null}
        </div>
        <button
          type="button"
          className="btn btn-primary"
          disabled={cycleBlocked || !!error}
          onClick={() => void runCycle()}
        >
          {cycleBlocked && <IconSpinner />}
          {cycleBlocked ? 'Cycle running…' : 'Run trading cycle'}
        </button>
      </section>

      {loading && !data ? (
        <div className="stat-grid" aria-hidden>
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="skeleton skeleton-card" />
          ))}
        </div>
      ) : data ? (
        <div className="stat-grid">
          <div className="stat-card">
            <div className="stat-card__label">Total equity</div>
            <div className="stat-card__value">{formatNaira(data.total_equity)}</div>
          </div>
          <div className="stat-card">
            <div className="stat-card__label">
              {data.tradingMode === 'live' ? 'Brokerage cash' : 'Cash'}
            </div>
            <div className="stat-card__value">{formatNaira(Number(data.portfolio?.cash_balance))}</div>
          </div>
          <div className="stat-card">
            <div className="stat-card__label">Market value</div>
            <div className="stat-card__value">{formatNaira(data.market_value)}</div>
          </div>
          <div className="stat-card">
            <div className="stat-card__label">Today&apos;s P&amp;L</div>
            <div
              className="stat-card__value"
              style={{ color: data.pnl_today >= 0 ? 'var(--status-ok)' : 'var(--status-bad)' }}
            >
              {formatNaira(data.pnl_today)}
            </div>
          </div>
        </div>
      ) : (
        <p className="empty-state">{error ? 'Could not load portfolio.' : 'Loading portfolio…'}</p>
      )}

      {performance.length > 0 ? (
        <div className="panel">
          <h2>Equity curve</h2>
          <ResponsiveContainer width="100%" height={280}>
            <AreaChart data={performance}>
              <defs>
                <linearGradient id="equityFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#38bdf8" stopOpacity={0.35} />
                  <stop offset="100%" stopColor="#38bdf8" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
              <XAxis dataKey="snapshot_date" stroke="var(--text-muted)" fontSize={11} tickMargin={8} />
              <YAxis
                stroke="var(--text-muted)"
                fontSize={11}
                tickFormatter={(v) => `${(v / 1e6).toFixed(1)}M`}
                width={48}
              />
              <Tooltip
                contentStyle={{
                  background: '#0f172a',
                  border: '1px solid #334155',
                  borderRadius: 8,
                  fontFamily: 'Fira Sans, sans-serif',
                }}
              />
              <Area
                type="monotone"
                dataKey="total_equity"
                stroke="#38bdf8"
                strokeWidth={2}
                fill="url(#equityFill)"
                dot={false}
                activeDot={{ r: 4 }}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      ) : data?.tradingMode === 'live' ? (
        <div className="panel">
          <h2>Equity curve</h2>
          <p className="muted" style={{ margin: 0 }}>
            No Wealth equity history yet. Points are recorded when you open Home or complete a live cycle.
          </p>
        </div>
      ) : null}

      {data && data.positions.length > 0 && (
        <div className="panel">
          <h2>Positions</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>Symbol</th>
                <th>Qty</th>
                <th>Avg cost</th>
                <th>Current</th>
                <th>Value</th>
              </tr>
            </thead>
            <tbody>
              {data.positions.map((p) => (
                <tr key={p.symbol}>
                  <td className="mono">
                    <Link className="symbol-link" to={`/symbol/${encodeURIComponent(p.symbol)}`}>
                      {p.symbol}
                    </Link>
                  </td>
                  <td className="mono">{Number(p.quantity).toLocaleString()}</td>
                  <td className="mono">{formatNaira(Number(p.avg_cost))}</td>
                  <td className="mono">{formatNaira(Number(p.current_price))}</td>
                  <td className="mono">{formatNaira(Number(p.market_value))}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {data && data.positions.length === 0 && (
        <div className="panel">
          <div className="empty-state">
            <div className="empty-state__icon"><IconShieldCheck size={22} /></div>
            <div>No open positions yet</div>
            <p className="muted" style={{ margin: 0, maxWidth: 360 }}>
              Run a trading cycle to generate signals and simulated fills in the sandbox.
            </p>
          </div>
        </div>
      )}
    </div>
  );
}
