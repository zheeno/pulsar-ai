'use client';

import { useEffect, useState } from 'react';
import { LineChart, Line, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid } from 'recharts';
import Nav from '@/components/Nav';
import { apiFetch } from '@/lib/api';

interface ParamSet { id: string; name: string; is_active: boolean }
interface BacktestRun {
  id: string; status: string; results?: {
    totalReturn: number; maxDrawdown: number; winRate: number; trades: number;
    equityCurve: { date: string; equity: number }[];
  };
}

export default function BacktestPage() {
  const [params, setParams] = useState<ParamSet[]>([]);
  const [run, setRun] = useState<BacktestRun | null>(null);
  const [form, setForm] = useState({ strategy_param_set_id: '', start_date: '2025-01-01', end_date: '2025-06-30' });
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    apiFetch('/strategy-params').then((r) => r.json()).then(setParams);
  }, []);

  async function startBacktest(e: React.FormEvent) {
    e.preventDefault();
    setLoading(true);
    const res = await apiFetch('/backtest', { method: 'POST', body: JSON.stringify(form) });
    const { runId } = await res.json();
    pollRun(runId);
  }

  async function pollRun(runId: string) {
    const interval = setInterval(async () => {
      const res = await apiFetch(`/backtest/${runId}`);
      const data = await res.json();
      setRun(data);
      if (data.status === 'completed' || data.status === 'failed') {
        clearInterval(interval);
        setLoading(false);
      }
    }, 2000);
  }

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 900, margin: '0 auto' }}>
        <h1>Backtest Runner</h1>
        <form onSubmit={startBacktest} style={{ background: '#1e293b', padding: 24, borderRadius: 8, marginBottom: 32 }}>
          <div style={{ marginBottom: 12 }}>
            <label style={{ display: 'block', marginBottom: 4, color: '#94a3b8' }}>Strategy Param Set</label>
            <select value={form.strategy_param_set_id} onChange={(e) => setForm({ ...form, strategy_param_set_id: e.target.value })}
              style={{ width: '100%', padding: 8, borderRadius: 6, background: '#0f172a', color: '#e2e8f0', border: '1px solid #334155' }}>
              <option value="">Select...</option>
              {params.map((p) => <option key={p.id} value={p.id}>{p.name}{p.is_active ? ' (active)' : ''}</option>)}
            </select>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginBottom: 16 }}>
            <div>
              <label style={{ display: 'block', marginBottom: 4, color: '#94a3b8' }}>Start Date</label>
              <input type="date" value={form.start_date} onChange={(e) => setForm({ ...form, start_date: e.target.value })}
                style={{ width: '100%', padding: 8, borderRadius: 6, background: '#0f172a', color: '#e2e8f0', border: '1px solid #334155', boxSizing: 'border-box' }} />
            </div>
            <div>
              <label style={{ display: 'block', marginBottom: 4, color: '#94a3b8' }}>End Date</label>
              <input type="date" value={form.end_date} onChange={(e) => setForm({ ...form, end_date: e.target.value })}
                style={{ width: '100%', padding: 8, borderRadius: 6, background: '#0f172a', color: '#e2e8f0', border: '1px solid #334155', boxSizing: 'border-box' }} />
            </div>
          </div>
          <button type="submit" disabled={loading || !form.strategy_param_set_id}
            style={{ background: loading ? '#475569' : '#3b82f6', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: loading ? 'not-allowed' : 'pointer' }}>
            {loading ? 'Running...' : 'Start Backtest'}
          </button>
        </form>

        {run && (
          <div style={{ background: '#1e293b', padding: 24, borderRadius: 8 }}>
            <h2>Results — {run.status}</h2>
            {run.results && (
              <>
                <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 16, marginBottom: 24 }}>
                  <Metric label="Total Return" value={`${run.results.totalReturn.toFixed(2)}%`} />
                  <Metric label="Max Drawdown" value={`${run.results.maxDrawdown.toFixed(2)}%`} />
                  <Metric label="Win Rate" value={`${run.results.winRate.toFixed(1)}%`} />
                  <Metric label="Trades" value={String(run.results.trades)} />
                </div>
                {run.results.equityCurve?.length > 0 && (
                  <ResponsiveContainer width="100%" height={300}>
                    <LineChart data={run.results.equityCurve}>
                      <CartesianGrid stroke="#334155" />
                      <XAxis dataKey="date" stroke="#94a3b8" fontSize={11} />
                      <YAxis stroke="#94a3b8" fontSize={11} tickFormatter={(v) => `${(v / 1e6).toFixed(1)}M`} />
                      <Tooltip contentStyle={{ background: '#1e293b', border: '1px solid #334155' }} />
                      <Line type="monotone" dataKey="equity" stroke="#a78bfa" dot={false} />
                    </LineChart>
                  </ResponsiveContainer>
                )}
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div style={{ color: '#94a3b8', fontSize: 13 }}>{label}</div>
      <div style={{ fontSize: 22, fontWeight: 700 }}>{value}</div>
    </div>
  );
}
