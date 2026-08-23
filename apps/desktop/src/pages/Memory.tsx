import { useEffect, useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { IconMemory } from '../components/Icons';
import { api } from '../lib/api';
import { useToast } from '../lib/toast';

interface MemoryRow {
  id: string;
  kind: string;
  symbol?: string | null;
  text: string;
  source: string;
  createdAt?: string;
  created_at?: string;
  score?: number;
}

function when(row: MemoryRow): string {
  return row.createdAt || row.created_at || '';
}

function truncate(text: string, n = 140): string {
  const t = text.replace(/\s+/g, ' ').trim();
  return t.length > n ? `${t.slice(0, n)}…` : t;
}

export default function MemoryPage() {
  const toast = useToast();
  const [rows, setRows] = useState<MemoryRow[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);

  async function loadList() {
    const data = await api<MemoryRow[]>('memory_list', { limit: 200 });
    setRows(data);
  }

  useEffect(() => {
    loadList()
      .catch((e) => toast.error(String(e), 'Could not load memory'))
      .finally(() => setLoaded(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- load once on mount
  }, []);

  async function onSearch(e: FormEvent) {
    e.preventDefault();
    const q = query.trim();
    setBusy(true);
    try {
      if (!q) {
        await loadList();
        return;
      }
      const data = await api<MemoryRow[]>('memory_search', { query: q, k: 40 });
      setRows(data);
    } catch (err) {
      toast.error(String(err), 'Search failed');
    } finally {
      setBusy(false);
    }
  }

  async function onDelete(id: string) {
    try {
      await api('memory_delete', { id });
      setRows((prev) => prev.filter((r) => r.id !== id));
    } catch (err) {
      toast.error(String(err), 'Could not delete');
    }
  }

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Memory</h1>
          <p>
            On-device lessons (cap 1000). Closed lots write a pattern note automatically — one loss
            is not a blacklist. When overnight desk review is on, repeated patterns compress into
            standing DREAM rules (still not a ticker ban, and they never raise minConfidence).
            Retrieved every cycle. Uses your existing LLM key for embeddings (OpenAI / OpenRouter);
            no external vector database.
          </p>
        </div>
        <form onSubmit={onSearch} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <input
            className="input"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search lessons…"
            aria-label="Search memory"
          />
          <button className="btn" type="submit" disabled={busy}>
            {busy ? 'Searching…' : 'Search'}
          </button>
        </form>
      </header>

      <div className="panel">
        {!loaded ? (
          <>
            <div className="skeleton skeleton-line" style={{ width: '25%' }} />
            <div className="skeleton skeleton-line" />
            <div className="skeleton skeleton-line" style={{ width: '70%' }} />
          </>
        ) : rows.length === 0 ? (
          <div className="empty-state">
            <div className="empty-state__icon"><IconMemory size={22} /></div>
            <div>No memories yet</div>
            <p className="muted" style={{ margin: 0 }}>
              Lessons appear when the agent upserts a durable note.
            </p>
          </div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>Kind</th>
                <th>Symbol</th>
                <th>Text</th>
                <th>Source</th>
                <th>Date</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id}>
                  <td>
                    <span className="status-pill status-pill--muted">{r.kind}</span>
                  </td>
                  <td className="mono">
                    {r.symbol ? (
                      <Link className="symbol-link" to={`/symbol/${encodeURIComponent(r.symbol)}`}>
                        {r.symbol}
                      </Link>
                    ) : (
                      '—'
                    )}
                  </td>
                  <td>{truncate(r.text)}</td>
                  <td className="muted">{r.source}</td>
                  <td className="muted" style={{ fontSize: 12 }}>{when(r)}</td>
                  <td>
                    <button className="btn btn-ghost" type="button" onClick={() => onDelete(r.id)}>
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
