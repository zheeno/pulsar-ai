import { useEffect, useState } from 'react';
import Nav from '../components/Nav';
import { api } from '../lib/api';

interface Signal {
  id: string;
  symbol: string;
  generated_at: string;
  action: string;
  confidence: number;
  rationale: string;
  model_name: string;
  executed: boolean;
  risk_policy_result: string;
}

export default function SignalsPage() {
  const [signals, setSignals] = useState<Signal[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => { loadSignals(); }, []);

  async function loadSignals() {
    const data = await api<Signal[]>('list_signals', { limit: 50 });
    setSignals(data);
  }

  async function generate() {
    setLoading(true);
    try {
      await api('generate_signals');
      await loadSignals();
    } catch (e) {
      alert(String(e));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 1200, margin: '0 auto' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 24 }}>
          <h1>Signals</h1>
          <button onClick={generate} disabled={loading} style={btnStyle}>
            {loading ? 'Generating...' : 'Generate Signals'}
          </button>
        </div>
        <table style={{ width: '100%', borderCollapse: 'collapse', background: '#1e293b', borderRadius: 8 }}>
          <thead>
            <tr style={{ borderBottom: '1px solid #334155', textAlign: 'left' }}>
              <th style={{ padding: 12 }}>Symbol</th>
              <th style={{ padding: 12 }}>Action</th>
              <th style={{ padding: 12 }}>Confidence</th>
              <th style={{ padding: 12 }}>Model</th>
              <th style={{ padding: 12 }}>Status</th>
              <th style={{ padding: 12 }}>Rationale</th>
            </tr>
          </thead>
          <tbody>
            {signals.map((s) => (
              <tr key={s.id} style={{ borderBottom: '1px solid #0f172a' }}>
                <td style={{ padding: 12 }}>{s.symbol}</td>
                <td style={{ padding: 12, color: s.action === 'BUY' ? '#22c55e' : s.action === 'SELL' ? '#ef4444' : '#94a3b8' }}>{s.action}</td>
                <td style={{ padding: 12 }}>{(s.confidence * 100).toFixed(0)}%</td>
                <td style={{ padding: 12, fontSize: 12 }}>{s.model_name}</td>
                <td style={{ padding: 12 }}>{s.executed ? 'Executed' : s.risk_policy_result}</td>
                <td style={{ padding: 12, fontSize: 13, color: '#94a3b8', maxWidth: 300 }}>{s.rationale}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

const btnStyle: React.CSSProperties = { background: '#3b82f6', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' };
