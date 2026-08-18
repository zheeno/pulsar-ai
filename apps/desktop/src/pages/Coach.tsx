import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { IconStrategy } from '../components/Icons';
import { CoachMarkdown } from '../components/CoachMarkdown';
import { api } from '../lib/api';
import { formatNaira } from '../lib/format';
import { useToast } from '../lib/toast';

type CoachDiffRow = {
  field: string;
  label: string;
  current: number;
  proposed: number;
  currentPct: number;
  proposedPct: number;
  rationale: string;
};

type CoachProposal = {
  needMoreContext?: boolean;
  clarifyingQuestions?: string[];
  summary?: string;
  patch?: Record<string, number>;
  rationale?: Record<string, string>;
  warnings?: string[];
  current?: Record<string, number>;
  diff?: CoachDiffRow[];
};

type ToolChip = {
  name: string;
  args?: unknown;
  ok?: boolean;
  summary?: string;
};

type TradeCard = {
  id: string;
  symbol: string;
  side: string;
  quantity?: number | null;
  notional?: number | null;
  rationale?: string | null;
  preview?: {
    price?: number;
    asOf?: string;
    estimatedCost?: number;
    warnings?: string[];
    minOrderNotional?: number;
    liveTradingEnabled?: boolean;
    haltNewBuys?: boolean;
  };
  status: string;
  result?: {
    ok?: boolean;
    riskPolicyResult?: string;
    warnings?: string[];
    error?: string;
    executed?: number;
  } | null;
};

type ChatTurn = {
  id: string;
  role: 'user' | 'assistant';
  text: string;
  proposal?: CoachProposal;
  toolTrace?: ToolChip[];
  trade?: TradeCard | null;
  error?: boolean;
  applied?: boolean;
  rejected?: boolean;
};

type CoachSession = {
  id: string;
  title: string;
  createdAt?: string;
  updatedAt?: string;
};

const PCT_BOUNDS: Record<string, { min: number; max: number }> = {
  max_position_pct: { min: 1, max: 50 },
  cycle_budget_pct: { min: 5, max: 100 },
  min_confidence_to_trade: { min: 40, max: 95 },
  max_daily_drawdown_pct: { min: 1, max: 100 },
  stop_loss_pct: { min: 1, max: 25 },
  take_profit_pct: { min: 2, max: 40 },
};

function uid() {
  return `c-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

const SUGGESTIONS = [
  "What's moving today?",
  'How is GTCO doing?',
  'Any news on MTNN?',
  'Tighten risk',
  'Sell half of GTCO if up 10%',
];

function excerptFromTurns(turns: ChatTurn[], message: string): string {
  const recent = turns.slice(-6).map((t) => `${t.role}: ${t.text}`.slice(0, 180));
  recent.push(`user: ${message.slice(0, 240)}`);
  return recent.join('\n');
}

function clampPct(field: string, value: number): number {
  const bounds = PCT_BOUNDS[field] || { min: 1, max: 100 };
  if (!Number.isFinite(value)) return bounds.min;
  return Math.min(bounds.max, Math.max(bounds.min, Math.round(value)));
}

function turnsFromSession(session: {
  messages?: {
    id: string;
    role: string;
    text: string;
    payload?: Record<string, unknown> | null;
  }[];
}): ChatTurn[] {
  return (session.messages || []).map((m) => {
    const payload = (m.payload || {}) as Record<string, unknown>;
    const diff = payload.diff as CoachDiffRow[] | undefined;
    const trade = payload.trade as TradeCard | undefined;
    const tradeResult = payload.tradeResult as TradeCard['result'] | undefined;
    const proposalId = payload.proposalId as string | undefined;
    let tradeCard = trade || null;
    if (tradeResult && proposalId) {
      tradeCard = {
        id: proposalId,
        symbol: '',
        side: '',
        status: tradeResult.ok ? 'executed' : 'blocked',
        result: tradeResult,
      };
    }
    return {
      id: m.id,
      role: m.role === 'assistant' ? 'assistant' : 'user',
      text: m.text,
      proposal: diff?.length
        ? {
            diff,
            warnings: (payload.warnings as string[]) || [],
            clarifyingQuestions: (payload.clarifyingQuestions as string[]) || [],
            needMoreContext: Boolean(payload.needMoreContext),
            patch: (payload.patch as Record<string, number>) || {},
            rationale: (payload.rationale as Record<string, string>) || {},
            current: (payload.current as Record<string, number>) || {},
          }
        : undefined,
      toolTrace: Array.isArray(payload.toolTrace) && (payload.toolTrace as ToolChip[]).length
        ? (payload.toolTrace as ToolChip[])
        : undefined,
      trade: tradeCard,
    };
  });
}

export default function CoachPage() {
  const toast = useToast();
  const [sessions, setSessions] = useState<CoachSession[]>([]);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('Thinking…');
  const [applyBusy, setApplyBusy] = useState<string | null>(null);
  const [tradeBusy, setTradeBusy] = useState<string | null>(null);
  const [checked, setChecked] = useState<Record<string, Record<string, boolean>>>({});
  const [editedPct, setEditedPct] = useState<Record<string, Record<string, number>>>({});
  const logRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [turns, busy]);

  async function loadSessions(preferId?: string | null) {
    const list = await api<CoachSession[]>('coach_list_sessions');
    setSessions(list);
    const session = await api<{
      id: string;
      title: string;
      messages?: {
        id: string;
        role: string;
        text: string;
        payload?: Record<string, unknown> | null;
      }[];
    }>('coach_get_session', preferId ? { id: preferId } : {});
    setSessionId(session.id);
    const nextTurns = turnsFromSession(session);
    setTurns(nextTurns);
    hydrateDiffState(nextTurns);
    return session.id;
  }

  function hydrateDiffState(nextTurns: ChatTurn[]) {
    const nextChecked: Record<string, Record<string, boolean>> = {};
    const nextPct: Record<string, Record<string, number>> = {};
    for (const t of nextTurns) {
      if (t.proposal?.diff?.length) {
        nextChecked[t.id] = Object.fromEntries(t.proposal.diff.map((row) => [row.field, true]));
        nextPct[t.id] = Object.fromEntries(
          t.proposal.diff.map((row) => [row.field, clampPct(row.field, row.proposedPct)]),
        );
      }
    }
    setChecked(nextChecked);
    setEditedPct(nextPct);
  }

  useEffect(() => {
    void loadSessions().catch((e) => toast.error(String(e), 'Coach'));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const canSend = useMemo(() => draft.trim().length > 0 && !busy, [draft, busy]);

  async function onSend(e?: FormEvent, preset?: string) {
    e?.preventDefault();
    const message = (preset ?? draft).trim();
    if (!message || busy) return;
    let sid = sessionId;
    if (!sid) {
      sid = await loadSessions();
    }
    if (!sid) {
      toast.error('Could not open a Coach session.', 'Coach');
      return;
    }

    const userTurn: ChatTurn = { id: uid(), role: 'user', text: message };
    setTurns((prev) => [...prev, userTurn]);
    setDraft('');
    setBusy(true);
    setStatus('Using tools…');

    try {
      await api('coach_turn', { sessionId: sid, message });
      await loadSessions(sid);
    } catch (err) {
      const text = String(err);
      toast.error(text, 'Coach');
      setTurns((prev) => [
        ...prev,
        { id: uid(), role: 'assistant', text, error: true },
      ]);
    } finally {
      setBusy(false);
      setStatus('Thinking…');
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }

  async function onNewChat() {
    const created = await api<{ id: string }>('coach_new_session');
    setSessionId(created.id);
    setTurns([]);
    setChecked({});
    setEditedPct({});
    const list = await api<CoachSession[]>('coach_list_sessions');
    setSessions(list);
    inputRef.current?.focus();
  }

  async function onDeleteSession(id: string) {
    await api('coach_delete_session', { id });
    if (sessionId === id) {
      await loadSessions();
    } else {
      setSessions(await api<CoachSession[]>('coach_list_sessions'));
    }
  }

  async function onApply(turn: ChatTurn) {
    const proposal = turn.proposal;
    if (!proposal || turn.applied || turn.rejected) return;
    const selectedFlags = checked[turn.id] || {};
    const selected: Record<string, number> = {};
    for (const row of proposal.diff || []) {
      if (!selectedFlags[row.field]) continue;
      const pct = clampPct(row.field, editedPct[turn.id]?.[row.field] ?? row.proposedPct);
      selected[row.field] = pct / 100;
    }
    if (Object.keys(selected).length === 0) {
      toast.warning('Select at least one parameter to apply.', 'Coach');
      return;
    }
    setApplyBusy(turn.id);
    try {
      await api('strategy_coach_apply', {
        selected,
        rationale: proposal.rationale,
        summary: proposal.summary || turn.text,
        chatExcerpt: excerptFromTurns(turns, turn.text),
      });
      setTurns((prev) => prev.map((t) => (t.id === turn.id ? { ...t, applied: true } : t)));
      toast.success('Strategy parameters were saved. Settings sliders now show the new values.', 'Coach');
    } catch (err) {
      toast.error(String(err), 'Coach');
    } finally {
      setApplyBusy(null);
    }
  }

  function onReject(turn: ChatTurn) {
    setTurns((prev) => prev.map((t) => (t.id === turn.id ? { ...t, rejected: true } : t)));
  }

  async function onConfirmTrade(turn: ChatTurn) {
    const trade = turn.trade;
    if (!trade?.id || trade.status !== 'proposed') return;
    setTradeBusy(trade.id);
    try {
      const result = await api<NonNullable<TradeCard['result']>>('coach_execute_trade', {
        proposalId: trade.id,
      });
      setTurns((prev) =>
        prev.map((t) =>
          t.id === turn.id
            ? {
                ...t,
                trade: {
                  ...trade,
                  status: result.ok ? 'executed' : 'blocked',
                  result,
                },
              }
            : t,
        ),
      );
      const note = result.ok
        ? `Filled (${result.riskPolicyResult || 'ok'})`
        : `Not filled: ${result.riskPolicyResult || result.error || 'blocked'}`;
      toast.info(note, 'Trade');
    } catch (err) {
      toast.error(String(err), 'Trade');
    } finally {
      setTradeBusy(null);
    }
  }

  async function onCancelTrade(turn: ChatTurn) {
    if (!turn.trade?.id || turn.trade.status !== 'proposed') return;
    try {
      await api('coach_cancel_trade', { proposalId: turn.trade.id });
      setTurns((prev) =>
        prev.map((t) =>
          t.id === turn.id && t.trade ? { ...t, trade: { ...t.trade, status: 'cancelled' } } : t,
        ),
      );
    } catch (err) {
      toast.error(String(err), 'Trade');
    }
  }

  return (
    <div className="page page--coach">
      <header className="page-header">
        <div>
          <h1>Coach</h1>
          <p>
            Ask about the market, history, news, and risk in plain language. Coach is a
            conversational copilot: it uses tools only when it needs a fact, and it can
            propose slider changes and trades. Nothing is saved or sent until you confirm.
          </p>
        </div>
      </header>

      <div className="coach-layout">
        <aside className="coach-sessions" aria-label="Chat sessions">
          <button type="button" className="btn btn-ghost" onClick={() => void onNewChat()} disabled={busy}>
            New chat
          </button>
          <ul className="coach-sessions__list">
            {sessions.map((s) => (
              <li key={s.id}>
                <button
                  type="button"
                  className={`coach-sessions__item${s.id === sessionId ? ' is-active' : ''}`}
                  disabled={busy}
                  onClick={() => void loadSessions(s.id)}
                >
                  {s.title || 'New chat'}
                </button>
                <button
                  type="button"
                  className="coach-sessions__delete"
                  aria-label={`Delete ${s.title || 'chat'}`}
                  onClick={() => void onDeleteSession(s.id)}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        </aside>

        <div className="panel coach-panel">
          <div className="coach-log" role="log" aria-live="polite" ref={logRef}>
            {turns.length === 0 && !busy ? (
              <div className="empty-state">
                <div className="empty-state__icon">
                  <IconStrategy size={22} />
                </div>
                <div>Ask Coach about the book, the tape, or a trade</div>
                <p className="muted" style={{ margin: 0 }}>
                  Market Q&amp;A, symbol history, news (when available), strategy sliders, and trade
                  proposals — numbers come from tools, not guesses.
                </p>
                <div className="coach-chips">
                  {SUGGESTIONS.map((s) => (
                    <button
                      key={s}
                      type="button"
                      className="coach-chip"
                      onClick={() => void onSend(undefined, s)}
                    >
                      {s}
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            {turns.map((turn) => (
              <article
                key={turn.id}
                className={`coach-bubble coach-bubble--${turn.role}${turn.error ? ' coach-bubble--error' : ''}`}
              >
                <div className="coach-bubble__role">
                  {turn.role === 'user' ? 'You' : 'Coach'}
                </div>
                {turn.role === 'assistant' ? (
                  <CoachMarkdown content={turn.text} className="coach-bubble__text" />
                ) : (
                  <p className="coach-bubble__text">{turn.text}</p>
                )}

                {turn.toolTrace?.length ? (
                  <ul className="coach-tools" aria-label="Tools used">
                    {turn.toolTrace.map((chip, i) => (
                      <li key={`${chip.name}-${i}`} className={chip.ok === false ? 'is-fail' : ''}>
                        <span className="mono">{chip.name}</span>
                        {chip.summary ? <span className="muted"> {chip.summary}</span> : null}
                      </li>
                    ))}
                  </ul>
                ) : null}

                {turn.proposal?.warnings?.length ? (
                  <ul className="coach-warnings">
                    {turn.proposal.warnings.map((w) => (
                      <li key={w}>{w}</li>
                    ))}
                  </ul>
                ) : null}

                {turn.proposal?.needMoreContext || (turn.proposal?.clarifyingQuestions?.length ?? 0) > 0 ? (
                  <ul className="coach-questions">
                    {(turn.proposal?.clarifyingQuestions || []).map((q) => (
                      <li key={q}>
                        <CoachMarkdown content={q} />
                      </li>
                    ))}
                  </ul>
                ) : null}

                {turn.proposal && (turn.proposal.diff?.length ?? 0) > 0 ? (
                  <div className="coach-diff">
                    <table className="data-table">
                      <thead>
                        <tr>
                          <th />
                          <th>Parameter</th>
                          <th>Current</th>
                          <th>Proposed</th>
                          <th>Why</th>
                        </tr>
                      </thead>
                      <tbody>
                        {turn.proposal.diff!.map((row) => (
                          <tr key={row.field}>
                            <td>
                              <input
                                type="checkbox"
                                checked={checked[turn.id]?.[row.field] ?? true}
                                disabled={turn.applied || turn.rejected || applyBusy === turn.id}
                                aria-label={`Apply ${row.label}`}
                                onChange={(e) =>
                                  setChecked((prev) => ({
                                    ...prev,
                                    [turn.id]: {
                                      ...(prev[turn.id] || {}),
                                      [row.field]: e.target.checked,
                                    },
                                  }))
                                }
                              />
                            </td>
                            <td>{row.label}</td>
                            <td className="mono">{row.currentPct}%</td>
                            <td>
                              <label className="coach-pct">
                                <input
                                  className="input coach-pct__input"
                                  type="number"
                                  min={PCT_BOUNDS[row.field]?.min ?? 1}
                                  max={PCT_BOUNDS[row.field]?.max ?? 100}
                                  step={1}
                                  value={editedPct[turn.id]?.[row.field] ?? row.proposedPct}
                                  disabled={turn.applied || turn.rejected || applyBusy === turn.id}
                                  aria-label={`Proposed ${row.label} percent`}
                                  onChange={(e) => {
                                    const next = Number(e.target.value);
                                    setEditedPct((prev) => ({
                                      ...prev,
                                      [turn.id]: {
                                        ...(prev[turn.id] || {}),
                                        [row.field]: Number.isFinite(next)
                                          ? next
                                          : (prev[turn.id]?.[row.field] ?? row.proposedPct),
                                      },
                                    }));
                                  }}
                                  onBlur={() =>
                                    setEditedPct((prev) => ({
                                      ...prev,
                                      [turn.id]: {
                                        ...(prev[turn.id] || {}),
                                        [row.field]: clampPct(
                                          row.field,
                                          prev[turn.id]?.[row.field] ?? row.proposedPct,
                                        ),
                                      },
                                    }))
                                  }
                                />
                                <span className="muted">%</span>
                              </label>
                            </td>
                            <td className="muted">{row.rationale || '—'}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    <div className="btn-row">
                      <button
                        type="button"
                        className="btn btn-primary"
                        disabled={turn.applied || turn.rejected || applyBusy === turn.id}
                        onClick={() => void onApply(turn)}
                      >
                        {turn.applied
                          ? 'Applied'
                          : applyBusy === turn.id
                            ? 'Saving…'
                            : 'Apply selected'}
                      </button>
                      <button
                        type="button"
                        className="btn btn-ghost"
                        disabled={turn.applied || turn.rejected || applyBusy === turn.id}
                        onClick={() => onReject(turn)}
                      >
                        {turn.rejected ? 'Rejected' : 'Reject'}
                      </button>
                    </div>
                  </div>
                ) : null}

                {turn.trade ? (
                  <div className="coach-trade">
                    <div className="coach-trade__head">
                      {turn.trade.side} {turn.trade.symbol}
                    </div>
                    <p className="muted" style={{ margin: '6px 0 0' }}>
                      Qty {turn.trade.quantity ?? '—'}
                      {turn.trade.preview?.estimatedCost != null
                        ? ` · est. ${formatNaira(turn.trade.preview.estimatedCost)}`
                        : ''}
                      {turn.trade.preview?.price != null
                        ? ` · px ${turn.trade.preview.price} as-of ${turn.trade.preview.asOf || '—'}`
                        : ''}
                    </p>
                    {turn.trade.preview?.warnings?.length ? (
                      <ul className="coach-warnings">
                        {turn.trade.preview.warnings.map((w) => (
                          <li key={w}>{w}</li>
                        ))}
                      </ul>
                    ) : null}
                    {turn.trade.result ? (
                      <p className="muted" style={{ marginBottom: 0 }}>
                        {turn.trade.result.ok
                          ? `Filled — ${turn.trade.result.riskPolicyResult || 'ok'}`
                          : `Not filled — ${turn.trade.result.riskPolicyResult || turn.trade.result.error || 'blocked'}`}
                      </p>
                    ) : null}
                    {turn.trade.status === 'proposed' ? (
                      <div className="btn-row">
                        <button
                          type="button"
                          className="btn btn-primary"
                          disabled={tradeBusy === turn.trade.id}
                          onClick={() => void onConfirmTrade(turn)}
                        >
                          {tradeBusy === turn.trade.id ? 'Submitting…' : 'Confirm trade'}
                        </button>
                        <button
                          type="button"
                          className="btn btn-ghost"
                          disabled={tradeBusy === turn.trade.id}
                          onClick={() => onCancelTrade(turn)}
                        >
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <p className="muted" style={{ marginBottom: 0 }}>
                        Status: {turn.trade.status}
                      </p>
                    )}
                  </div>
                ) : null}
              </article>
            ))}
            {busy ? (
              <article className="coach-bubble coach-bubble--assistant" aria-busy="true">
                <div className="coach-bubble__role">Coach</div>
                <p className="muted" style={{ margin: 0 }}>{status}</p>
              </article>
            ) : null}
          </div>

          <form className="coach-composer" onSubmit={(e) => void onSend(e)} aria-busy={busy}>
            <input
              ref={inputRef}
              className="input"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder="Ask about movers, a symbol, news, risk, or a trade…"
              aria-label="Message Coach"
              disabled={busy}
            />
            <button className="btn btn-primary" type="submit" disabled={!canSend}>
              {busy ? 'Sending…' : 'Send'}
            </button>
          </form>
        </div>
      </div>
    </div>
  );
}
