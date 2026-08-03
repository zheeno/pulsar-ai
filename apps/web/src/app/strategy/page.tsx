'use client';

import { useEffect, useState } from 'react';
import Nav from '@/components/Nav';
import { apiFetch } from '@/lib/api';

interface ParamSet {
  id: string;
  name: string;
  max_position_pct: number;
  max_daily_trades: number;
  stop_loss_pct: number;
  min_confidence_to_trade: number;
  max_daily_drawdown_pct: number;
  position_size_pct: number;
  is_active: boolean;
}

export default function StrategyPage() {
  const [params, setParams] = useState<ParamSet[]>([]);
  const [form, setForm] = useState({
    name: '', max_position_pct: 0.1, max_daily_trades: 5, stop_loss_pct: 0.05,
    min_confidence_to_trade: 0.65, max_daily_drawdown_pct: 0.03, position_size_pct: 0.05,
  });

  useEffect(() => { loadParams(); }, []);

  async function loadParams() {
    const res = await apiFetch('/strategy-params');
    setParams(await res.json());
  }

  async function createParamSet(e: React.FormEvent) {
    e.preventDefault();
    await apiFetch('/strategy-params', { method: 'POST', body: JSON.stringify(form) });
    setForm({ ...form, name: '' });
    loadParams();
  }

  async function activate(id: string) {
    await apiFetch(`/strategy-params/${id}/activate`, { method: 'POST' });
    loadParams();
  }

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 800, margin: '0 auto' }}>
        <h1>Strategy Configuration</h1>

        <form onSubmit={createParamSet} style={{ background: '#1e293b', padding: 24, borderRadius: 8, marginBottom: 32 }}>
          <h2 style={{ marginTop: 0 }}>Create New Version</h2>
          <Field label="Name" value={form.name} onChange={(v) => setForm({ ...form, name: v })} />
          <Field label="Max Position %" value={form.max_position_pct} onChange={(v) => setForm({ ...form, max_position_pct: Number(v) })} type="number" step="0.01" />
          <Field label="Max Daily Trades" value={form.max_daily_trades} onChange={(v) => setForm({ ...form, max_daily_trades: Number(v) })} type="number" />
          <Field label="Stop Loss %" value={form.stop_loss_pct} onChange={(v) => setForm({ ...form, stop_loss_pct: Number(v) })} type="number" step="0.01" />
          <Field label="Min Confidence" value={form.min_confidence_to_trade} onChange={(v) => setForm({ ...form, min_confidence_to_trade: Number(v) })} type="number" step="0.01" />
          <Field label="Max Daily Drawdown %" value={form.max_daily_drawdown_pct} onChange={(v) => setForm({ ...form, max_daily_drawdown_pct: Number(v) })} type="number" step="0.01" />
          <Field label="Position Size %" value={form.position_size_pct} onChange={(v) => setForm({ ...form, position_size_pct: Number(v) })} type="number" step="0.01" />
          <button type="submit" style={{ background: '#3b82f6', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' }}>
            Create Version
          </button>
        </form>

        <h2>Version History</h2>
        {params.map((p) => (
          <div key={p.id} style={{ background: '#1e293b', padding: 16, borderRadius: 8, marginBottom: 8, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
            <div>
              <strong>{p.name}</strong> {p.is_active && <span style={{ color: '#22c55e', marginLeft: 8 }}>ACTIVE</span>}
              <div style={{ fontSize: 13, color: '#94a3b8', marginTop: 4 }}>
                Max pos: {(Number(p.max_position_pct) * 100).toFixed(0)}% | Min conf: {Number(p.min_confidence_to_trade)} | Daily trades: {p.max_daily_trades}
              </div>
            </div>
            {!p.is_active && (
              <button onClick={() => activate(p.id)} style={{ background: '#334155', color: '#e2e8f0', border: 'none', padding: '6px 14px', borderRadius: 4, cursor: 'pointer' }}>
                Activate
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function Field({ label, value, onChange, type = 'text', step }: { label: string; value: string | number; onChange: (v: string) => void; type?: string; step?: string }) {
  return (
    <div style={{ marginBottom: 12 }}>
      <label style={{ display: 'block', marginBottom: 4, fontSize: 14, color: '#94a3b8' }}>{label}</label>
      <input type={type} step={step} value={value} onChange={(e) => onChange(e.target.value)}
        style={{ width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid #334155', background: '#0f172a', color: '#e2e8f0', boxSizing: 'border-box' }} />
    </div>
  );
}
