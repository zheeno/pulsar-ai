import { useState } from 'react';
import Nav from '../components/Nav';
import { api } from '../lib/api';

export default function BacktestPage() {
  const [strategyId, setStrategyId] = useState('');
  const [startDate, setStartDate] = useState('2025-01-01');
  const [endDate, setEndDate] = useState('2025-06-01');
  const [runId, setRunId] = useState('');
  const [result, setResult] = useState<Record<string, unknown> | null>(null);
  const [loading, setLoading] = useState(false);

  async function loadStrategy() {
    const s = await api<{ id: string }>('get_strategy');
    setStrategyId(s.id);
  }

  async function startBacktest() {
    setLoading(true);
    try {
      if (!strategyId) await loadStrategy();
      const id = strategyId || (await api<{ id: string }>('get_strategy')).id;
      const rid = await api<string>('start_backtest', {
        strategyParamSetId: id,
        startDate,
        endDate,
      });
      setRunId(rid);
      pollResult(rid);
    } catch (e) {
      alert(String(e));
      setLoading(false);
    }
  }

  async function pollResult(rid: string) {
    for (let i = 0; i < 60; i++) {
      await new Promise((r) => setTimeout(r, 2000));
      const run = await api<{ status: string; results?: Record<string, unknown> }>('get_backtest', { runId: rid });
      if (run.status === 'completed' || run.status === 'failed') {
        setResult(run.results || { status: run.status });
        setLoading(false);
        return;
      }
    }
    setLoading(false);
  }

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 800, margin: '0 auto' }}>
        <h1>Backtest</h1>
        <div style={{ background: '#1e293b', padding: 24, borderRadius: 8, marginTop: 16 }}>
          <label style={labelStyle}>Start Date</label>
          <input type="date" style={inputStyle} value={startDate} onChange={(e) => setStartDate(e.target.value)} />
          <label style={labelStyle}>End Date</label>
          <input type="date" style={inputStyle} value={endDate} onChange={(e) => setEndDate(e.target.value)} />
          <button onClick={startBacktest} disabled={loading} style={{ ...btnStyle, marginTop: 16 }}>
            {loading ? 'Running...' : 'Start Backtest'}
          </button>
        </div>
        {runId && <p style={{ color: '#94a3b8', marginTop: 12 }}>Run ID: {runId}</p>}
        {result && (
          <div style={{ background: '#1e293b', padding: 24, borderRadius: 8, marginTop: 16 }}>
            <h2 style={{ marginTop: 0 }}>Results</h2>
            <pre style={{ fontSize: 13, overflow: 'auto' }}>{JSON.stringify(result, null, 2)}</pre>
          </div>
        )}
      </div>
    </div>
  );
}

const labelStyle: React.CSSProperties = { display: 'block', marginTop: 12, marginBottom: 4, fontSize: 13, color: '#94a3b8' };
const inputStyle: React.CSSProperties = { width: '100%', padding: 10, borderRadius: 6, border: '1px solid #334155', background: '#0f172a', color: '#e2e8f0' };
const btnStyle: React.CSSProperties = { background: '#3b82f6', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' };
