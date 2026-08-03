'use client';

import { useEffect, useState } from 'react';
import Nav from '@/components/Nav';
import { apiFetch } from '@/lib/api';

interface Signal {
  id: string;
  symbol: string;
  action: string;
  confidence: number;
  rationale: string;
  risk_policy_result: string;
  executed: boolean;
  generated_at: string;
}

export default function SignalsPage() {
  const [signals, setSignals] = useState<Signal[]>([]);
  const [filter, setFilter] = useState({ symbol: '', action: '' });

  useEffect(() => { loadSignals(); }, [filter]);

  async function loadSignals() {
    const params = new URLSearchParams();
    if (filter.symbol) params.set('symbol', filter.symbol);
    if (filter.action) params.set('action', filter.action);
    const res = await apiFetch(`/signals?${params}`);
    const data = await res.json();
    setSignals(data.data || []);
  }

  const actionColor = (action: string) => {
    if (action === 'BUY') return '#22c55e';
    if (action === 'SELL') return '#ef4444';
    return '#94a3b8';
  };

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 1000, margin: '0 auto' }}>
        <h1>Signal Feed</h1>
        <div style={{ display: 'flex', gap: 12, marginBottom: 24 }}>
          <input placeholder="Filter symbol" value={filter.symbol} onChange={(e) => setFilter({ ...filter, symbol: e.target.value })}
            style={inputStyle} />
          <select value={filter.action} onChange={(e) => setFilter({ ...filter, action: e.target.value })} style={inputStyle}>
            <option value="">All actions</option>
            <option value="BUY">BUY</option>
            <option value="SELL">SELL</option>
            <option value="HOLD">HOLD</option>
          </select>
        </div>
        {signals.map((s) => (
          <div key={s.id} style={{ background: '#1e293b', padding: 16, borderRadius: 8, marginBottom: 12 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 8 }}>
              <span>
                <strong style={{ color: actionColor(s.action) }}>{s.action}</strong> {s.symbol}
                <span style={{ color: '#94a3b8', marginLeft: 8 }}>confidence: {(Number(s.confidence) * 100).toFixed(0)}%</span>
              </span>
              <span style={{ fontSize: 12, color: '#64748b' }}>{new Date(s.generated_at).toLocaleString()}</span>
            </div>
            <p style={{ margin: '8px 0', color: '#cbd5e1' }}>{s.rationale}</p>
            <div style={{ fontSize: 13, color: '#94a3b8' }}>
              Risk: {s.risk_policy_result} {s.executed ? '✓ Executed' : ''}
            </div>
          </div>
        ))}
        {signals.length === 0 && <p style={{ color: '#64748b' }}>No signals yet. Run a cycle from the dashboard.</p>}
      </div>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  padding: '8px 12px', borderRadius: 6, border: '1px solid #334155', background: '#0f172a', color: '#e2e8f0',
};
