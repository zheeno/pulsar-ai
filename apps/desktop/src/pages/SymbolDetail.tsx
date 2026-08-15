import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { IconSpinner } from '../components/Icons';
import { api } from '../lib/api';
import { cryptoBaseAsset, formatCryptoQty, formatMoney } from '../lib/format';
import { useSession } from '../lib/session';
import { useToast } from '../lib/toast';

type PricePoint = {
  date: string;
  price: number;
  changePercent?: number | null;
  volume?: number | null;
};

type SymbolDetail = {
  symbol: string;
  name?: string | null;
  sector?: string | null;
  found: boolean;
  latest?: PricePoint | null;
  prices: PricePoint[];
  position?: { quantity: number; avgCost: number } | null;
  signals: {
    id: string;
    generatedAt: string;
    action: string;
    confidence: number;
    rationale: string;
    modelName: string;
    executed: boolean;
    riskPolicyResult: string;
  }[];
  trades: {
    id: string;
    side: string;
    quantity: number;
    fillPrice: number;
    simulatedFee: number;
    executedAt: string;
    resultingCashBalance: number;
  }[];
  pulseQuote?: {
    price?: number;
    changePercent?: number | null;
    volume?: number | null;
    marketCap?: number | null;
    peRatio?: number | null;
  } | null;
  needsPulsePrices?: boolean;
};

type SymbolDetailPulse = {
  symbol: string;
  prices: PricePoint[];
  pulseQuote?: SymbolDetail['pulseQuote'];
  error?: string | null;
};

export default function SymbolDetailPage() {
  const { symbol: rawSymbol } = useParams();
  const toast = useToast();
  const { activeModule } = useSession();
  const isCrypto = activeModule === 'crypto';
  const money = (n: number | null | undefined) => formatMoney(n, isCrypto ? 'crypto' : 'stocks');
  const symbol = (rawSymbol || '').toUpperCase();
  const displayBase = isCrypto ? cryptoBaseAsset(symbol) : symbol;
  const [data, setData] = useState<SymbolDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [pulseRefreshing, setPulseRefreshing] = useState(false);

  useEffect(() => {
    if (!symbol) return;
    let cancelled = false;
    setLoading(true);
    setPulseRefreshing(false);
    setData(null);

    api<SymbolDetail>('symbol_detail', { symbol })
      .then((d) => {
        if (cancelled) return;
        setData(d);
        setLoading(false);

        if (!d.needsPulsePrices) return;

        setPulseRefreshing(true);
        api<SymbolDetailPulse>('symbol_detail_pulse', { symbol })
          .then((pulse) => {
            if (cancelled || pulse.error) return;
            setData((prev) => {
              if (!prev || prev.symbol !== symbol) return prev;
              return {
                ...prev,
                found: prev.found || pulse.prices.length > 0,
                prices: pulse.prices.length > 0 ? pulse.prices : prev.prices,
                latest: pulse.prices.length > 0
                  ? pulse.prices[pulse.prices.length - 1]
                  : prev.latest,
                pulseQuote: pulse.pulseQuote ?? prev.pulseQuote,
                needsPulsePrices: false,
              };
            });
          })
          .catch(() => {
            /* local data already shown */
          })
          .finally(() => {
            if (!cancelled) setPulseRefreshing(false);
          });
      })
      .catch((e) => {
        if (!cancelled) {
          setData(null);
          setLoading(false);
          toast.error(String(e), 'Symbol');
        }
      });

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reload on symbol change
  }, [symbol]);

  const price = data?.pulseQuote?.price ?? data?.latest?.price;
  const change = data?.pulseQuote?.changePercent ?? data?.latest?.changePercent;

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <p className="muted" style={{ margin: '0 0 8px' }}>
            <Link to="/" className="symbol-link">Home</Link>
            <span aria-hidden> / </span>
            <Link to="/trades" className="symbol-link">Trades</Link>
            <span aria-hidden> / </span>
            <span className="mono" title={symbol}>
              {displayBase}
              {isCrypto ? <span className="muted">/USDT</span> : null}
            </span>
          </p>
          <h1 className="mono" style={{ marginBottom: 4 }} title={symbol}>
            {displayBase}
            {isCrypto ? (
              <span className="muted" style={{ fontSize: '0.65em', fontWeight: 500, marginLeft: 8 }}>
                /USDT
              </span>
            ) : null}
          </h1>
          <p style={{ margin: 0 }}>
            {data?.name || (loading ? '…' : 'Unknown instrument')}
            {data?.sector ? <span className="muted"> · {data.sector}</span> : null}
            {pulseRefreshing ? (
              <span className="muted" style={{ marginLeft: 8, display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                <IconSpinner /> refreshing history
              </span>
            ) : null}
          </p>
        </div>
        {!loading && price != null ? (
          <div style={{ textAlign: 'right' }}>
            <div className="stat-card__value" style={{ fontSize: '1.5rem' }}>{money(price)}</div>
            {change != null ? (
              <div className={change >= 0 ? 'text-ok' : 'text-bad'} style={{ fontWeight: 600 }}>
                {change >= 0 ? '+' : ''}{change.toFixed(2)}%
              </div>
            ) : null}
          </div>
        ) : null}
      </header>

      {loading ? (
        <div className="panel">
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }} className="muted">
            <IconSpinner /> Loading {symbol}…
          </div>
        </div>
      ) : !data || !data.found ? (
        <div className="panel">
          <div className="empty-state">
            <div>No data for {symbol}</div>
            <p className="muted" style={{ margin: 0 }}>Try running a cycle to ingest market data.</p>
          </div>
        </div>
      ) : (
        <>
          {(data.pulseQuote || data.position) && (
            <div className="stat-grid">
              {data.pulseQuote?.volume != null && (
                <div className="stat-card">
                  <div className="stat-card__label">Volume</div>
                  <div className="stat-card__value mono" style={{ fontSize: '1.1rem' }}>
                    {Number(data.pulseQuote.volume).toLocaleString()}
                  </div>
                </div>
              )}
              {data.pulseQuote?.marketCap != null && (
                <div className="stat-card">
                  <div className="stat-card__label">Market cap</div>
                  <div className="stat-card__value mono" style={{ fontSize: '1.1rem' }}>
                    {money(data.pulseQuote.marketCap)}
                  </div>
                </div>
              )}
              {data.position && (
                <>
                  <div className="stat-card">
                    <div className="stat-card__label">Position qty</div>
                    <div className="stat-card__value mono" style={{ fontSize: '1.1rem' }}>
                      {isCrypto
                        ? formatCryptoQty(data.position.quantity, symbol)
                        : Number(data.position.quantity).toLocaleString()}
                    </div>
                  </div>
                  <div className="stat-card">
                    <div className="stat-card__label">{isCrypto ? 'Avg cost (USDT)' : 'Avg cost'}</div>
                    <div className="stat-card__value mono" style={{ fontSize: '1.1rem' }}>
                      {money(data.position.avgCost)}
                    </div>
                  </div>
                </>
              )}
            </div>
          )}

          {data.prices.length > 0 && (
            <div className="panel">
              <h2>Price history</h2>
              <ResponsiveContainer width="100%" height={260}>
                <AreaChart data={data.prices}>
                  <defs>
                    <linearGradient id="symbolFill" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#38bdf8" stopOpacity={0.35} />
                      <stop offset="100%" stopColor="#38bdf8" stopOpacity={0.02} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
                  <XAxis dataKey="date" stroke="var(--text-muted)" fontSize={11} tickMargin={8} />
                  <YAxis
                    stroke="var(--text-muted)"
                    fontSize={11}
                    width={56}
                    tickFormatter={(v) => Number(v).toFixed(0)}
                  />
                  <Tooltip
                    contentStyle={{
                      background: '#0f172a',
                      border: '1px solid #334155',
                      borderRadius: 8,
                    }}
                  />
                  <Area
                    type="monotone"
                    dataKey="price"
                    stroke="#38bdf8"
                    strokeWidth={2}
                    fill="url(#symbolFill)"
                    dot={false}
                  />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          )}

          <div className="panel">
            <h2>Signals</h2>
            {data.signals.length === 0 ? (
              <p className="muted" style={{ margin: 0 }}>No signals for this symbol yet.</p>
            ) : (
              <table className="data-table">
                <thead>
                  <tr>
                    <th>When</th>
                    <th>Action</th>
                    <th>Confidence</th>
                    <th>Status</th>
                    <th>Rationale</th>
                  </tr>
                </thead>
                <tbody>
                  {data.signals.map((s) => (
                    <tr key={s.id}>
                      <td className="muted" style={{ fontSize: 12 }}>{s.generatedAt}</td>
                      <td className={s.action === 'BUY' ? 'text-ok' : s.action === 'SELL' ? 'text-bad' : 'muted'}>
                        {s.action}
                      </td>
                      <td className="mono">{(s.confidence * 100).toFixed(0)}%</td>
                      <td>{s.executed ? 'Executed' : s.riskPolicyResult}</td>
                      <td className="muted" style={{ fontSize: 13, maxWidth: 280 }}>{s.rationale}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="panel">
            <h2>Trades</h2>
            {data.trades.length === 0 ? (
              <p className="muted" style={{ margin: 0 }}>No sandbox trades for this symbol yet.</p>
            ) : (
              <table className="data-table">
                <thead>
                  <tr>
                    <th>When</th>
                    <th>Side</th>
                    <th>Qty</th>
                    <th>{isCrypto ? 'Fill (USDT)' : 'Fill'}</th>
                    <th>Fee</th>
                  </tr>
                </thead>
                <tbody>
                  {data.trades.map((t) => (
                    <tr key={t.id}>
                      <td className="muted" style={{ fontSize: 12 }}>{t.executedAt}</td>
                      <td className={t.side === 'BUY' ? 'text-ok' : 'text-bad'}>{t.side}</td>
                      <td className="mono">
                        {isCrypto
                          ? formatCryptoQty(t.quantity, symbol)
                          : Number(t.quantity).toLocaleString()}
                      </td>
                      <td className="mono">{money(t.fillPrice)}</td>
                      <td className="mono">{money(t.simulatedFee)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}
    </div>
  );
}
