'use client';

import { useEffect, useState } from 'react';
import Nav from '@/components/Nav';
import { apiFetch } from '@/lib/api';

interface Trade {
  id: string;
  symbol: string;
  side: string;
  quantity: number;
  fill_price: number;
  simulated_fee: number;
  simulated_slippage_bps: number;
  executed_at: string;
  resulting_cash_balance: number;
  rationale?: string;
}

export default function TradesPage() {
  const [trades, setTrades] = useState<Trade[]>([]);

  useEffect(() => { loadTrades(); }, []);

  async function loadTrades() {
    const res = await apiFetch('/trades');
    const data = await res.json();
    setTrades(data.data || []);
  }

  const formatNaira = (n: number) => `₦${n.toLocaleString('en-NG', { maximumFractionDigits: 2 })}`;

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 1100, margin: '0 auto' }}>
        <h1>Trade History</h1>
        <table style={{ width: '100%', borderCollapse: 'collapse', marginTop: 16 }}>
          <thead>
            <tr style={{ borderBottom: '1px solid #334155', textAlign: 'left' }}>
              <th style={{ padding: 10 }}>Time</th>
              <th style={{ padding: 10 }}>Symbol</th>
              <th style={{ padding: 10 }}>Side</th>
              <th style={{ padding: 10 }}>Qty</th>
              <th style={{ padding: 10 }}>Fill Price</th>
              <th style={{ padding: 10 }}>Fee</th>
              <th style={{ padding: 10 }}>Slippage</th>
            </tr>
          </thead>
          <tbody>
            {trades.map((t) => (
              <tr key={t.id} style={{ borderBottom: '1px solid #1e293b' }}>
                <td style={{ padding: 10, fontSize: 13 }}>{new Date(t.executed_at).toLocaleString()}</td>
                <td style={{ padding: 10 }}>{t.symbol}</td>
                <td style={{ padding: 10, color: t.side === 'BUY' ? '#22c55e' : '#ef4444' }}>{t.side}</td>
                <td style={{ padding: 10 }}>{Number(t.quantity).toLocaleString()}</td>
                <td style={{ padding: 10 }}>{formatNaira(Number(t.fill_price))}</td>
                <td style={{ padding: 10 }}>{formatNaira(Number(t.simulated_fee))}</td>
                <td style={{ padding: 10 }}>{Number(t.simulated_slippage_bps)} bps</td>
              </tr>
            ))}
          </tbody>
        </table>
        {trades.length === 0 && <p style={{ color: '#64748b', marginTop: 24 }}>No trades yet.</p>}
      </div>
    </div>
  );
}
