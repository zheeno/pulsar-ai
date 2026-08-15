import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { IconTrades } from '../components/Icons';
import { api } from '../lib/api';
import { cryptoBaseAsset, formatCryptoQty, formatMoney } from '../lib/format';
import { useSession } from '../lib/session';
import { useToast } from '../lib/toast';

interface Trade {
  id: string;
  symbol: string;
  side: string;
  quantity: number;
  fill_price: number;
  simulated_fee: number;
  executed_at: string;
  resulting_cash_balance?: number | null;
  venue?: string;
  status?: string;
  rejection_reason?: string | null;
}

export default function TradesPage() {
  const toast = useToast();
  const { activeModule } = useSession();
  const [trades, setTrades] = useState<Trade[]>([]);
  const [loaded, setLoaded] = useState(false);
  const isCrypto = activeModule === 'crypto';
  const money = (n: number | null | undefined) => formatMoney(n, isCrypto ? 'crypto' : 'stocks');

  useEffect(() => {
    setLoaded(false);
    api<Trade[]>('list_trades', { limit: 50 })
      .then((data) => {
        setTrades(data);
      })
      .catch((e) => toast.error(String(e), 'Could not load trades'))
      .finally(() => setLoaded(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reload when module changes
  }, [activeModule]);

  const hasLive = trades.some((t) => t.venue === 'wealth');

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Trades</h1>
          <p>
            {isCrypto
              ? 'Simulated USDT fills from the crypto sandbox. Live crypto brokers are not available yet.'
              : hasLive
              ? 'Sandbox fills and Coronation Wealth live orders.'
              : 'Simulated fills from the sandbox execution engine. Connect Wealth in Settings for live orders.'}
          </p>
        </div>
      </header>

      <div className="panel">
        {!loaded ? (
          <>
            <div className="skeleton skeleton-line" style={{ width: '25%' }} />
            <div className="skeleton skeleton-line" />
            <div className="skeleton skeleton-line" style={{ width: '70%' }} />
          </>
        ) : trades.length === 0 ? (
          <div className="empty-state">
            <div className="empty-state__icon"><IconTrades size={22} /></div>
            <div>No trades yet</div>
            <p className="muted" style={{ margin: 0 }}>Run a cycle from Home to create fills.</p>
          </div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>Symbol</th>
                <th>Side</th>
                <th>Qty</th>
                <th>{isCrypto ? 'Fill (USDT)' : 'Fill price'}</th>
                <th>Fee</th>
                <th>Venue</th>
                <th>Cash after</th>
                <th>Time</th>
              </tr>
            </thead>
            <tbody>
              {trades.map((t) => (
                <tr key={t.id}>
                  <td className="mono">
                    <Link
                      className="symbol-link"
                      to={`/symbol/${encodeURIComponent(t.symbol)}`}
                      title={t.symbol}
                    >
                      {isCrypto ? cryptoBaseAsset(t.symbol) : t.symbol}
                      {isCrypto ? (
                        <span className="muted" style={{ marginLeft: 6, fontWeight: 400 }}>
                          /USDT
                        </span>
                      ) : null}
                    </Link>
                  </td>
                  <td className={t.side === 'BUY' ? 'text-ok' : 'text-bad'}>{t.side}</td>
                  <td className="mono">
                    {isCrypto
                      ? formatCryptoQty(Number(t.quantity), t.symbol)
                      : Number(t.quantity).toLocaleString(undefined, { maximumFractionDigits: 0 })}
                  </td>
                  <td className="mono">{money(Number(t.fill_price))}</td>
                  <td className="mono">{money(Number(t.simulated_fee))}</td>
                  <td>
                    <span className={`status-pill ${t.venue === 'wealth' ? 'status-pill--ok' : 'status-pill--muted'}`}>
                      {t.venue === 'wealth' ? 'Live' : 'Sandbox'}
                      {t.status && t.status !== 'executed' ? ` · ${t.status}` : ''}
                    </span>
                  </td>
                  <td className="mono">
                    {t.resulting_cash_balance != null
                      ? money(Number(t.resulting_cash_balance))
                      : '—'}
                  </td>
                  <td className="muted" style={{ fontSize: 12 }}>{t.executed_at}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
