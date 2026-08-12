import { useEffect, useState } from 'react';
import Nav from '../components/Nav';
import { api } from '../lib/api';

export default function StrategyPage() {
  const [strategy, setStrategy] = useState<Record<string, unknown> | null>(null);

  useEffect(() => {
    api<Record<string, unknown>>('get_strategy').then(setStrategy).catch(() => {});
  }, []);

  return (
    <div>
      <Nav />
      <div style={{ padding: 24, maxWidth: 800, margin: '0 auto' }}>
        <h1>Strategy Parameters</h1>
        {strategy && (
          <div style={{ background: '#1e293b', padding: 24, borderRadius: 8, marginTop: 16 }}>
            <Row label="Name" value={String(strategy.name)} />
            <Row label="Max Position %" value={`${(Number(strategy.max_position_pct) * 100).toFixed(0)}%`} />
            <Row label="Max Daily Trades" value={String(strategy.max_daily_trades)} />
            <Row label="Stop Loss %" value={`${(Number(strategy.stop_loss_pct) * 100).toFixed(0)}%`} />
            <Row label="Min Confidence" value={`${(Number(strategy.min_confidence_to_trade) * 100).toFixed(0)}%`} />
            <Row label="Max Daily Drawdown" value={`${(Number(strategy.max_daily_drawdown_pct) * 100).toFixed(0)}%`} />
            <Row label="Position Size %" value={`${(Number(strategy.position_size_pct) * 100).toFixed(0)}%`} />
            <Row label="Allowed Symbols" value={Array.isArray(strategy.allowed_symbols) ? (strategy.allowed_symbols as string[]).join(', ') : 'All'} />
          </div>
        )}
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: 'flex', justifyContent: 'space-between', padding: '8px 0', borderBottom: '1px solid #334155' }}>
      <span style={{ color: '#94a3b8' }}>{label}</span>
      <span>{value}</span>
    </div>
  );
}
