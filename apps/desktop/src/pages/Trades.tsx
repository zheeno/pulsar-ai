import { useEffect, useState } from 'react';
import Nav from '../components/Nav';
import { api } from '../lib/api';

interface Trade {
  id: string;
  symbol: string;
  side: string;
  quantity: number;
  fill_price: number;
  simulated_fee: number;
  executed_at: string;
  resulting_cash_balance: number;
}

export default function TradesPage() {
  const [trades, setTrades] = useState<Trade[]>([]);

  useEffect(() => {
    api<Trade[]>('list_trades', { limit: 50 }).then(setTrades).catch(() => {});
  }, []);

  const formatNaira = (n: number) => `₦${n.toLocaleString('en-NG', { maximumFractionDigits: 0 })}`;

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 1200, margin: '0 auto' }}>
        <h1>Trades</h1>
        <table style={{ width: '100%', borderCollapse: 'collapse', background: '#1e293b', borderRadius: 8, marginTop: 16 }}>
          <thead>
            <tr style={{ borderBottom: '1px solid #334155', textAlign: 'left' }}>
              <th style={{ padding: 12 }}>Symbol</th>
              <th style={{ padding: 12 }}>Side</th>
              <th style={{ padding: 12 }}>Qty</th>
              <th style={{ padding: 12 }}>Fill Price</th>
              <th style={{ padding: 12 }}>Fee</th>
              <th style={{ padding: 12 }}>Cash After</th>
              <th style={{ padding: 12 }}>Time</th>
            </tr>
          </thead>
          <tbody>
            {trades.map((t) => (
              <tr key={t.id} style={{ borderBottom: '1px solid #0f172a' }}>
                <td style={{ padding: 12 }}>{t.symbol}</td>
                <td style={{ padding: 12, color: t.side === 'BUY' ? '#22c55e' : '#ef4444' }}>{t.side}</td>
                <td style={{ padding: 12 }}>{Number(t.quantity).toLocaleString()}</td>
                <td style={{ padding: 12 }}>{formatNaira(Number(t.fill_price))}</td>
                <td style={{ padding: 12 }}>{formatNaira(Number(t.simulated_fee))}</td>
                <td style={{ padding: 12 }}>{formatNaira(Number(t.resulting_cash_balance))}</td>
                <td style={{ padding: 12, fontSize: 12, color: '#94a3b8' }}>{t.executed_at}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
