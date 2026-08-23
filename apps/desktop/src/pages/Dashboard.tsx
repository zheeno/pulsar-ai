import { useEffect, useId, useMemo, useState } from 'react';
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { DeskAdviceCard } from '../components/DeskAdviceCard';
import { IconShieldAlert, IconShieldCheck, IconSpinner } from '../components/Icons';
import { api, type DeskAdvice, type PortfolioData } from '../lib/api';
import { formatNaira } from '../lib/format';
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
    unrealized_pnl: Number((raw as { unrealized_pnl?: number; unrealizedPnl?: number }).unrealized_pnl
      ?? (raw as { unrealizedPnl?: number }).unrealizedPnl
      ?? 0),
    quotesAsOf: (raw as { quotesAsOf?: string; syncedAt?: string }).quotesAsOf
      ?? (raw as { syncedAt?: string }).syncedAt
      ?? null,
    stale: Boolean((raw as { stale?: boolean }).stale),
    tradingMode: (raw as { tradingMode?: string }).tradingMode || 'sandbox',
    assetClass: (raw as { assetClass?: string }).assetClass
      || ((raw as { brokerId?: string }).brokerId === 'busha' ? 'crypto' : 'stocks'),
    cryptoSession: (raw as { cryptoSession?: string }).cryptoSession ?? undefined,
    brokerId: (raw as { brokerId?: string }).brokerId ?? null,
    brokerName: (raw as { brokerName?: string }).brokerName ?? null,
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

type EquityPoint = {
  recorded_at?: string;
  snapshot_date?: string;
  total_equity: number;
};

function pointInstant(p: EquityPoint): string {
  return p.recorded_at || (p.snapshot_date ? `${p.snapshot_date}T00:00:00Z` : '');
}

function calendarDay(iso: string): string {
  return iso.slice(0, 10);
}

function formatCurveTick(iso: string, points: EquityPoint[]): string {
  const day = calendarDay(iso);
  const sameDayCount = points.filter((p) => calendarDay(pointInstant(p)) === day).length;
  const d = new Date(iso.includes('T') ? iso : `${iso}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return iso;
  if (sameDayCount > 1) {
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

function formatCurveTooltipTime(iso: string): string {
  const d = new Date(iso.includes('T') ? iso : `${iso}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatQuoteClock(iso?: string | null): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export default function DashboardPage() {
  const toast = useToast();
  const { running: cycleRunning } = useCycle();
  const [data, setData] = useState<PortfolioData | null>(null);
  const [performance, setPerformance] = useState<EquityPoint[]>([]);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [market, setMarket] = useState<MarketStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [cycleBusy, setCycleBusy] = useState(false);
  const [cycleConfirmOpen, setCycleConfirmOpen] = useState(false);
  const [deskAdvice, setDeskAdvice] = useState<DeskAdvice | null>(null);
  const cycleConfirmTitleId = useId();

  useEffect(() => {
    void loadData();
    const interval = setInterval(() => void loadData(), 30000);
    return () => clearInterval(interval);
  }, []);

  useEffect(() => {
    if (!cycleConfirmOpen) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape' && !cycleBusy) setCycleConfirmOpen(false);
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [cycleConfirmOpen, cycleBusy]);

  useEffect(() => {
    if (loading && !data) return;
    const phase = market?.phase ?? 'closed';
    const bypass = market != null && market.marketHoursEnforced === false;
    const poll = bypass || phase === 'open' || phase === 'post_close';
    if (!poll) return;

    const intervalMs = bypass || phase === 'open' ? 60_000 : 300_000;
    let cancelled = false;
    let inFlight = false;

    async function tickQuotes() {
      if (document.hidden || inFlight) return;
      inFlight = true;
      try {
        const q = await api<{
          positions: PortfolioData['positions'];
          total_equity: number;
          market_value: number;
          pnl_today: number;
          unrealized_pnl?: number;
          quotesAsOf?: string;
          stale?: boolean;
          tradingMode?: string;
          portfolio?: { cash_balance?: number };
        }>('portfolio_quotes');
        if (cancelled) return;
        setData((prev) => {
          if (!prev) return prev;
          const stale = Boolean(q.stale);
          const quoteLots = (q.positions ?? []).some((p) => Number(p.quantity) > 0);
          const keep = (next: number | undefined, current: number, rejectZero = false) => {
            if (stale) return current;
            if (next == null || !Number.isFinite(Number(next))) return current;
            const n = Number(next);
            if (rejectZero && n === 0 && current !== 0) return current;
            return n;
          };
          const cashRaw = Number(q.portfolio?.cash_balance);
          const cashCollapsed =
            Number.isFinite(cashRaw)
            && cashRaw === 0
            && Number(prev.portfolio?.cash_balance) > 0
            && (stale || (Number(q.market_value) === 0 && quoteLots));
          const quoteMode = q.tradingMode || 'sandbox';
          const liveQuote = prev.tradingMode === 'live' && quoteMode === 'live';
          const cash = cashCollapsed || stale || !Number.isFinite(cashRaw) || (prev.tradingMode === 'live' && !liveQuote)
            ? Number(prev.portfolio?.cash_balance ?? 0)
            : cashRaw;
          if (prev.tradingMode === 'live') {
            const positions = liveQuote && quoteLots && !stale
              ? (q.positions ?? prev.positions)
              : prev.positions;
            const liveMvFromLots = positions.reduce((sum, p) => sum + Number(p.market_value || 0), 0);
            const liveMv = positions.length > 0
              ? liveMvFromLots
              : (Number(prev.market_value) > 0 ? Number(prev.market_value) : liveMvFromLots);
            return {
              ...prev,
              positions,
              market_value: liveMv,
              total_equity: cash + liveMv,
              pnl_today: keep(q.pnl_today, prev.pnl_today),
              unrealized_pnl: keep(q.unrealized_pnl, prev.unrealized_pnl ?? 0),
              quotesAsOf: q.quotesAsOf ?? prev.quotesAsOf,
              stale,
              portfolio: {
                ...prev.portfolio,
                cash_balance: cash,
              },
            };
          }
          const marketValue = keep(q.market_value, prev.market_value, quoteLots);
          const wipe = !stale
            && Number(q.total_equity) === 0
            && Number(prev.total_equity) > 0
            && (quoteLots || Number(prev.market_value) > 0);
          if (wipe) {
            return {
              ...prev,
              quotesAsOf: q.quotesAsOf ?? prev.quotesAsOf,
              stale: true,
            };
          }
          return {
            ...prev,
            positions: stale ? prev.positions : (q.positions ?? prev.positions),
            market_value: marketValue,
            total_equity: Number.isFinite(Number(q.total_equity)) && !stale
              ? Number(q.total_equity)
              : cash + marketValue,
            pnl_today: keep(q.pnl_today, prev.pnl_today),
            unrealized_pnl: keep(q.unrealized_pnl, prev.unrealized_pnl ?? 0),
            quotesAsOf: q.quotesAsOf ?? prev.quotesAsOf,
            stale,
            portfolio: {
              ...prev.portfolio,
              cash_balance: cash,
            },
          };
        });
      } catch {
        /* keep last marks */
      } finally {
        inFlight = false;
      }
    }

    void tickQuotes();
    const id = window.setInterval(() => void tickQuotes(), intervalMs);
    const onVis = () => {
      if (!document.hidden) void tickQuotes();
    };
    document.addEventListener('visibilitychange', onVis);
    return () => {
      cancelled = true;
      window.clearInterval(id);
      document.removeEventListener('visibilitychange', onVis);
    };
  }, [loading, market?.phase, market?.marketHoursEnforced]);

  async function loadData() {
    try {
      const [portfolioRaw, usageData, marketData, advice] = await Promise.all([
        api<RawPortfolio>('portfolio_default'),
        api<Usage>('usage_ngx_pulse'),
        api<MarketStatus>('market_status'),
        api<DeskAdvice>('desk_advice').catch(() => null),
      ]);
      const portfolio = normalizePortfolio(portfolioRaw);
      setData((prev) => {
        if (portfolio.brokerId && prev?.brokerId && portfolio.brokerId !== prev.brokerId) {
          return portfolio;
        }
        if (prev?.tradingMode === 'live' && portfolio.tradingMode !== 'live') {
          return prev;
        }
        if (portfolio.tradingMode === 'live') return portfolio;
        if (!prev?.quotesAsOf) return portfolio;
        const cash = Number(portfolio.portfolio?.cash_balance ?? prev.portfolio?.cash_balance);
        return {
          ...portfolio,
          portfolio: {
            ...portfolio.portfolio,
            cash_balance: cash,
          },
          positions: prev.positions,
          market_value: prev.market_value,
          total_equity: cash + prev.market_value,
          pnl_today: prev.pnl_today,
          unrealized_pnl: prev.unrealized_pnl,
          quotesAsOf: prev.quotesAsOf,
          stale: prev.stale,
        };
      });
      setUsage(usageData);
      setMarket(marketData);
      setDeskAdvice(advice);
      setError(null);
      const venue = portfolio.tradingMode === 'live' ? (portfolio.brokerId || 'wealth') : 'sandbox';
      const perf = await api<EquityPoint[]>(
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
      const result = await api<{
        liveDisabled?: boolean;
        executed?: number;
        signals?: number;
      }>('cycle_run');
      if (result?.liveDisabled) {
        toast.warning('Live trading is off in Settings — signals were generated only.', 'Cycle');
      }
      void loadData();
    } catch (e) {
      const msg = String(e);
      if (msg.toLowerCase().includes('already running')) {
        toast.warning(msg, 'Cycle');
      }
    } finally {
      setCycleBusy(false);
      setCycleConfirmOpen(false);
    }
  }

  function closeCycleConfirm() {
    if (cycleBusy) return;
    setCycleConfirmOpen(false);
  }

  const cycleBlocked = cycleRunning || cycleBusy;

  const chartData = useMemo(
    () =>
      performance.map((p) => ({
        ...p,
        recorded_at: pointInstant(p),
      })),
    [performance],
  );

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
      const broker = data.brokerName || 'broker';
      if (data.cryptoSession === 'expiredNeedsReconnect' || data.tradingMode === 'paused') {
        return {
          level: 'warn' as const,
          title: 'Busha session expired',
          sub: 'Live crypto trading is paused. You are still in crypto mode — reconnect Busha to resume (not NGX sandbox).',
          live: false,
        };
      }
      return {
        level: 'ok' as const,
        title: 'Systems healthy',
        sub: live
          ? (data.assetClass === 'crypto'
            ? 'Busha is connected. Home shows NGN cash and crypto holdings; cycles place live Busha transfers when live trading is on.'
            : (data.tradingVerified
            ? `NGX Pulse is active. Home shows your ${broker} brokerage cash and holdings; cycles can place live market orders.`
            : (data.wealthStatus?.message
              || `${broker} connected — Home shows brokerage cash and holdings. Complete trading verification to enable live orders.`)))
          : data.wealthError
            ? `${broker} connected but portfolio sync failed: ${data.wealthError}`
            : data.assetClass === 'crypto'
              ? 'Busha crypto mode is on. Enable live trading in Settings to place orders.'
              : 'NGX Pulse session is active and your sandbox portfolio is ready. Connect a live broker in Settings to go live.',
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
            {market && data?.assetClass !== 'crypto' ? (
              <>
                <span className="status-hero__meta-sep" aria-hidden>·</span>
                <span className={`status-pill ${marketChipClass}`} style={{ padding: '2px 8px', fontSize: '0.72rem' }}>
                  {market.phase === 'open' && <span className="live-dot" aria-hidden />}
                  {marketPhaseLabel(market.phase)}
                </span>
              </>
            ) : data?.assetClass === 'crypto' ? (
              <>
                <span className="status-hero__meta-sep" aria-hidden>·</span>
                <span
                  className={`status-pill ${
                    data.cryptoSession === 'expiredNeedsReconnect' || data.tradingMode === 'paused'
                      ? 'status-pill--warn'
                      : 'status-pill--ok'
                  }`}
                  style={{ padding: '2px 8px', fontSize: '0.72rem' }}
                >
                  {data.cryptoSession === 'expiredNeedsReconnect' || data.tradingMode === 'paused'
                    ? 'Crypto · Expired'
                    : 'Crypto · 24h'}
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
                    ? (data.tradingVerified === false ? (data.brokerName || 'Broker') : 'Live trader')
                    : data.tradingMode === 'paused'
                      ? 'Paused'
                      : 'Sandbox'}
                </span>
              </>
            ) : null}
          </div>
          <h1 className="status-hero__title">{status.title}</h1>
          <p className="status-hero__sub">{status.sub}</p>
          {(data?.cryptoSession === 'expiredNeedsReconnect' || data?.tradingMode === 'paused') ? (
            <p className="status-hero__sub" style={{ marginTop: 8 }}>
              <Link to="/settings">Reconnect Busha in Settings</Link>
            </p>
          ) : null}
          {market && data?.assetClass !== 'crypto' ? (
            <p className="status-hero__market muted">
              {market.nowWat}
              {market.pulseStatus ? ` · Pulse: ${market.pulseStatus}` : ''}
              {!market.marketHoursEnforced ? ' · Hours bypassed (dev)' : ''}
              {data?.quotesAsOf
                ? ` · Quoted ${formatQuoteClock(data.quotesAsOf)}${data.stale ? ' · Stale' : data.tradingMode === 'live' ? ` · ${data.brokerName || 'Broker'}` : ' · Pulse'}`
                : ''}
            </p>
          ) : data?.assetClass === 'crypto' ? (
            <p className="status-hero__market muted">
              Busha crypto book in NGN
              {data?.quotesAsOf
                ? ` · Quoted ${formatQuoteClock(data.quotesAsOf)}${data.stale ? ' · Stale' : ' · Busha'}`
                : ''}
            </p>
          ) : null}
        </div>
        <button
          type="button"
          className="btn btn-primary"
          disabled={cycleBlocked || !!error}
          onClick={() => setCycleConfirmOpen(true)}
        >
          {cycleBlocked && <IconSpinner />}
          {cycleBlocked ? 'Cycle running…' : 'Run trading cycle'}
        </button>
      </section>

      <DeskAdviceCard advice={deskAdvice} />

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
            <div className="muted" style={{ fontSize: 12, marginTop: 6 }}>
              Unrealized vs cost{' '}
              <span style={{ color: (data.unrealized_pnl ?? 0) >= 0 ? 'var(--status-ok)' : 'var(--status-bad)' }}>
                {formatNaira(data.unrealized_pnl ?? 0)}
              </span>
            </div>
          </div>
        </div>
      ) : (
        <p className="empty-state">{error ? 'Could not load portfolio.' : 'Loading portfolio…'}</p>
      )}

      {chartData.length > 0 ? (
        <div className="panel">
          <h2>Equity curve</h2>
          <ResponsiveContainer width="100%" height={280}>
            <AreaChart data={chartData}>
              <defs>
                <linearGradient id="equityFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#38bdf8" stopOpacity={0.35} />
                  <stop offset="100%" stopColor="#38bdf8" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
              <XAxis
                dataKey="recorded_at"
                stroke="var(--text-muted)"
                fontSize={11}
                tickMargin={8}
                tickFormatter={(iso: string) => formatCurveTick(iso, chartData)}
              />
              <YAxis
                stroke="var(--text-muted)"
                fontSize={11}
                tickFormatter={(v) => `${(v / 1e6).toFixed(1)}M`}
                width={48}
              />
              <Tooltip
                content={({ active, payload, label }) => {
                  if (!active || !payload?.length) return null;
                  const equity = Number(payload[0].value);
                  return (
                    <div
                      style={{
                        background: '#0f172a',
                        border: '1px solid #334155',
                        borderRadius: 8,
                        padding: '8px 10px',
                        fontFamily: 'Fira Sans, sans-serif',
                      }}
                    >
                      <div style={{ color: 'var(--text-muted)', fontSize: 12 }}>
                        {formatCurveTooltipTime(String(label ?? ''))}
                      </div>
                      <div>{formatNaira(equity)}</div>
                    </div>
                  );
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
            No Wealth equity history yet. Points are recorded after each live trading cycle.
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

      {cycleConfirmOpen ? (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) closeCycleConfirm();
          }}
        >
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby={cycleConfirmTitleId}
          >
            <div className="modal__header">
              <div>
                <h2 id={cycleConfirmTitleId}>Run trading cycle?</h2>
                <p className="muted" style={{ margin: '6px 0 0', fontSize: 13, lineHeight: 1.45 }}>
                  {data?.cryptoSession === 'expiredNeedsReconnect' || data?.tradingMode === 'paused'
                    ? 'Busha session is expired. This cycle will not place live crypto orders until you reconnect.'
                    : data?.tradingMode === 'live' && data?.assetClass === 'crypto'
                      ? 'This runs a full crypto cycle on Busha. If live trading is on in Settings, approved signals can place real NGN transfers.'
                      : data?.tradingMode === 'live'
                        ? `This runs a full cycle on ${data.brokerName || 'your live broker'}. If live trading is on in Settings, approved signals can place real market orders.`
                        : 'This generates signals and sandbox fills only. Connect a live broker and enable live trading in Settings to place real orders.'}
                </p>
              </div>
              <button
                type="button"
                className="modal__close"
                aria-label="Close"
                onClick={closeCycleConfirm}
                disabled={cycleBusy}
              >
                ×
              </button>
            </div>
            <div className="btn-row">
              <button
                type="button"
                className="btn btn-ghost"
                disabled={cycleBusy}
                onClick={closeCycleConfirm}
              >
                Cancel
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={cycleBusy || cycleRunning}
                onClick={() => void runCycle()}
              >
                {cycleBusy ? <IconSpinner /> : null}
                {cycleBusy ? 'Starting…' : 'Run cycle'}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
