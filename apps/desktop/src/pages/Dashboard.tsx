import { useEffect, useState } from 'react';
import { LineChart, Line, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid } from 'recharts';
import Nav from '../components/Nav';
import { api, type PortfolioData } from '../lib/api';

export default function DashboardPage() {
  const [data, setData] = useState<PortfolioData | null>(null);
  const [performance, setPerformance] = useState<{ snapshot_date: string; total_equity: number }[]>([]);
  const [usage, setUsage] = useState<{ daily: number; limit: number | null; remaining: number | null; authMode?: string } | null>(null);

  useEffect(() => {
    loadData();
    const interval = setInterval(loadData, 30000);
    return () => clearInterval(interval);
  }, []);

  async function loadData() {
    try {
      const portfolio = await api<PortfolioData>('portfolio_default');
      setData(portfolio);
      if (portfolio.portfolio?.id) {
        const perf = await api<{ snapshot_date: string; total_equity: number }[]>('portfolio_performance', { id: portfolio.portfolio.id });
        setPerformance(perf);
      }
      const usageData = await api<typeof usage>('usage_ngx_pulse');
      setUsage(usageData);
    } catch {
      /* portfolio may not exist yet */
    }
  }

  async function runCycle() {
    try {
      const result = await api<{ signals: number; executed: number; warnings: string[] }>('cycle_run');
      alert(`Cycle complete: ${result.signals} signals, ${result.executed} executed`);
      loadData();
    } catch (e) {
      alert(`Cycle failed: ${e}`);
    }
  }

  const formatNaira = (n: number) => `₦${n.toLocaleString('en-NG', { maximumFractionDigits: 0 })}`;

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 1200, margin: '0 auto' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 24 }}>
          <h1>Portfolio Overview</h1>
          <button onClick={runCycle} style={{ background: '#22c55e', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' }}>
            Run Cycle
          </button>
        </div>

        {data && (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 16, marginBottom: 32 }}>
            <StatCard label="Total Equity" value={formatNaira(data.total_equity)} />
            <StatCard label="Cash Balance" value={formatNaira(Number(data.portfolio.cash_balance))} />
            <StatCard label="Market Value" value={formatNaira(data.market_value)} />
            <StatCard label="Today's P&L" value={formatNaira(data.pnl_today)} color={data.pnl_today >= 0 ? '#22c55e' : '#ef4444'} />
          </div>
        )}

        {usage && (
          <div style={{ background: '#1e293b', padding: 16, borderRadius: 8, marginBottom: 24 }}>
            NGX Pulse ({usage.authMode ?? 'api_key'}): {usage.daily}
            {usage.limit != null ? `/${usage.limit} requests today (${usage.remaining} remaining)` : ' requests today (unlimited session auth)'}
          </div>
        )}

        {performance.length > 0 && (
          <div style={{ background: '#1e293b', padding: 24, borderRadius: 8, marginBottom: 32 }}>
            <h2 style={{ marginTop: 0 }}>Equity Curve</h2>
            <ResponsiveContainer width="100%" height={300}>
              <LineChart data={performance}>
                <CartesianGrid stroke="#334155" />
                <XAxis dataKey="snapshot_date" stroke="#94a3b8" fontSize={12} />
                <YAxis stroke="#94a3b8" fontSize={12} tickFormatter={(v) => `${(v / 1e6).toFixed(1)}M`} />
                <Tooltip contentStyle={{ background: '#1e293b', border: '1px solid #334155' }} />
                <Line type="monotone" dataKey="total_equity" stroke="#3b82f6" dot={false} />
              </LineChart>
            </ResponsiveContainer>
          </div>
        )}

        {data && data.positions.length > 0 && (
          <div style={{ background: '#1e293b', padding: 24, borderRadius: 8 }}>
            <h2 style={{ marginTop: 0 }}>Positions</h2>
            <table style={{ width: '100%', borderCollapse: 'collapse' }}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155', textAlign: 'left' }}>
                  <th style={{ padding: 8 }}>Symbol</th>
                  <th style={{ padding: 8 }}>Qty</th>
                  <th style={{ padding: 8 }}>Avg Cost</th>
                  <th style={{ padding: 8 }}>Current</th>
                  <th style={{ padding: 8 }}>Value</th>
                </tr>
              </thead>
              <tbody>
                {data.positions.map((p) => (
                  <tr key={p.symbol} style={{ borderBottom: '1px solid #1e293b' }}>
                    <td style={{ padding: 8 }}>{p.symbol}</td>
                    <td style={{ padding: 8 }}>{Number(p.quantity).toLocaleString()}</td>
                    <td style={{ padding: 8 }}>{formatNaira(Number(p.avg_cost))}</td>
                    <td style={{ padding: 8 }}>{formatNaira(Number(p.current_price))}</td>
                    <td style={{ padding: 8 }}>{formatNaira(Number(p.market_value))}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function StatCard({ label, value, color }: { label: string; value: string; color?: string }) {
  return (
    <div style={{ background: '#1e293b', padding: 20, borderRadius: 8 }}>
      <div style={{ color: '#94a3b8', fontSize: 14, marginBottom: 4 }}>{label}</div>
      <div style={{ fontSize: 24, fontWeight: 700, color: color || '#e2e8f0' }}>{value}</div>
    </div>
  );
}
