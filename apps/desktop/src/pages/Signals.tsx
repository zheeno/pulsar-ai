import { useEffect, useState } from 'react';
import { IconSignals, IconSpinner } from '../components/Icons';
import { api, type AppSettings } from '../lib/api';
import { useToast } from '../lib/toast';

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

function formatGeneratedAt(raw?: string | null): string {
  if (!raw) return '—';
  const normalized = raw.includes('T') ? raw : raw.replace(' ', 'T') + (raw.endsWith('Z') ? '' : 'Z');
  const d = new Date(normalized);
  if (Number.isNaN(d.getTime())) return raw;
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export default function SignalsPage() {
  const toast = useToast();
  const [signals, setSignals] = useState<Signal[]>([]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [cryptoMode, setCryptoMode] = useState(false);

  useEffect(() => {
    void loadSignals();
    void api<AppSettings>('settings_get')
      .then((s) => setCryptoMode(s.assetClass === 'crypto'))
      .catch(() => undefined);
  }, []);

  async function loadSignals() {
    try {
      const data = await api<Signal[]>('list_signals', { limit: 50 });
      setSignals(data);
    } catch (e) {
      toast.error(String(e), 'Could not load signals');
    } finally {
      setLoaded(true);
    }
  }

  async function generate() {
    setLoading(true);
    try {
      const result = await api<{ count: number; universeSize?: number }>('generate_signals');
      await loadSignals();
      const size = result.universeSize != null ? ` Universe ${result.universeSize}.` : '';
      toast.success(`Fresh signals are ready.${size}`, 'Signals generated');
    } catch (e) {
      toast.error(String(e), 'Generate failed');
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Signals</h1>
          <p>Model recommendations for {cryptoMode ? 'Busha NGN crypto pairs' : 'the NGX sandbox'}. Generate a fresh batch when ready.</p>
        </div>
        <button type="button" className="btn btn-primary" onClick={() => void generate()} disabled={loading}>
          {loading && <IconSpinner />}
          {loading ? 'Generating…' : 'Generate signals'}
        </button>
      </header>

      <div className="panel">
        {!loaded ? (
          <>
            <div className="skeleton skeleton-line" style={{ width: '30%' }} />
            <div className="skeleton skeleton-line" />
            <div className="skeleton skeleton-line" style={{ width: '80%' }} />
          </>
        ) : signals.length === 0 ? (
          <div className="empty-state">
            <div className="empty-state__icon"><IconSignals size={22} /></div>
            <div>No signals yet</div>
            <p className="muted" style={{ margin: 0 }}>Generate a batch to populate this list.</p>
            <button type="button" className="btn btn-primary" onClick={() => void generate()} disabled={loading}>
              Generate signals
            </button>
          </div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>Generated</th>
                <th>Symbol</th>
                <th>Action</th>
                <th>Confidence</th>
                <th>Model</th>
                <th>Status</th>
                <th>Rationale</th>
              </tr>
            </thead>
            <tbody>
              {signals.map((s) => (
                <tr key={s.id}>
                  <td className="muted mono" style={{ fontSize: 12, whiteSpace: 'nowrap' }}>
                    {formatGeneratedAt(s.generated_at)}
                  </td>
                  <td className="mono">{s.symbol}</td>
                  <td className={s.action === 'BUY' ? 'text-ok' : s.action === 'SELL' ? 'text-bad' : 'muted'}>
                    {s.action}
                  </td>
                  <td className="mono">{(s.confidence * 100).toFixed(0)}%</td>
                  <td className="muted" style={{ fontSize: 12 }}>{s.model_name}</td>
                  <td>{s.executed ? 'Executed' : s.risk_policy_result}</td>
                  <td className="muted" style={{ fontSize: 13, maxWidth: 280 }}>{s.rationale}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
