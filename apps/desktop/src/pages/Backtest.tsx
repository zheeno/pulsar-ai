import { useId, useState } from 'react';
import { IconBacktest, IconSpinner } from '../components/Icons';
import { api } from '../lib/api';
import { useToast } from '../lib/toast';

export default function BacktestPage() {
  const toast = useToast();
  const startId = useId();
  const endId = useId();
  const [strategyId, setStrategyId] = useState('');
  const [startDate, setStartDate] = useState('2025-01-01');
  const [endDate, setEndDate] = useState('2025-06-01');
  const [runId, setRunId] = useState('');
  const [result, setResult] = useState<Record<string, unknown> | null>(null);
  const [loading, setLoading] = useState(false);

  async function loadStrategy() {
    const s = await api<{ id: string }>('get_strategy');
    setStrategyId(s.id);
    return s.id;
  }

  async function startBacktest() {
    setLoading(true);
    setResult(null);
    toast.info('Starting backtest…', 'Backtest');
    try {
      const id = strategyId || (await loadStrategy());
      const rid = await api<string>('start_backtest', {
        strategyParamSetId: id,
        startDate,
        endDate,
      });
      setRunId(rid);
      toast.info(`Running ${rid}…`, 'Backtest');
      await pollResult(rid);
    } catch (e) {
      toast.error(String(e), 'Backtest failed');
      setLoading(false);
    }
  }

  async function pollResult(rid: string) {
    for (let i = 0; i < 60; i++) {
      await new Promise((r) => setTimeout(r, 2000));
      const run = await api<{ status: string; results?: Record<string, unknown> }>('get_backtest', { runId: rid });
      if (run.status === 'completed' || run.status === 'failed') {
        setResult(run.results || { status: run.status });
        if (run.status === 'completed') {
          toast.success('Results are ready.', 'Backtest completed');
        } else {
          toast.error('The run finished with a failure status.', 'Backtest failed');
        }
        setLoading(false);
        return;
      }
    }
    toast.warning('Timed out waiting for backtest results.', 'Backtest');
    setLoading(false);
  }

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Backtest</h1>
          <p>Replay strategy parameters over a historical window in the local sandbox.</p>
        </div>
      </header>

      <div className="panel">
        <label className="label" htmlFor={startId}>Start date</label>
        <input id={startId} className="input" type="date" value={startDate} onChange={(e) => setStartDate(e.target.value)} />
        <label className="label" htmlFor={endId}>End date</label>
        <input id={endId} className="input" type="date" value={endDate} onChange={(e) => setEndDate(e.target.value)} />
        <div className="btn-row">
          <button type="button" className="btn btn-primary" onClick={() => void startBacktest()} disabled={loading}>
            {loading && <IconSpinner />}
            {loading ? 'Running…' : 'Start backtest'}
          </button>
        </div>
      </div>

      {runId && <p className="muted mono">Run ID: {runId}</p>}

      {result ? (
        <div className="panel">
          <h2>Results</h2>
          <pre className="pre-log">{JSON.stringify(result, null, 2)}</pre>
        </div>
      ) : !loading && (
        <div className="panel">
          <div className="empty-state">
            <div className="empty-state__icon"><IconBacktest size={22} /></div>
            <div>No backtest results yet</div>
            <p className="muted" style={{ margin: 0 }}>Choose a date range and start a run.</p>
          </div>
        </div>
      )}
    </div>
  );
}
