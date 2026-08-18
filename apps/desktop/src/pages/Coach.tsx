import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { IconStrategy } from '../components/Icons';
import { api } from '../lib/api';
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
  needMoreContext: boolean;
  clarifyingQuestions: string[];
  summary: string;
  patch: Record<string, number>;
  rationale: Record<string, string>;
  warnings: string[];
  current: Record<string, number>;
  diff: CoachDiffRow[];
};

type ChatTurn = {
  id: string;
  role: 'user' | 'assistant';
  text: string;
  proposal?: CoachProposal;
  error?: boolean;
  applied?: boolean;
  rejected?: boolean;
};

function uid() {
  return `c-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

const FIELD_PCT_BOUNDS: Record<string, { min: number; max: number }> = {
  max_position_pct: { min: 1, max: 50 },
  cycle_budget_pct: { min: 5, max: 100 },
  min_confidence_to_trade: { min: 40, max: 95 },
  max_daily_drawdown_pct: { min: 1, max: 100 },
  stop_loss_pct: { min: 1, max: 25 },
  take_profit_pct: { min: 2, max: 40 },
};

function excerptFromTurns(turns: ChatTurn[], message: string): string {
  const recent = turns.slice(-6).map((t) => `${t.role}: ${t.text}`.slice(0, 180));
  recent.push(`user: ${message.slice(0, 240)}`);
  return recent.join('\n');
}

function clampPct(field: string, value: number): number {
  const bounds = FIELD_PCT_BOUNDS[field] || { min: 1, max: 100 };
  if (!Number.isFinite(value)) return bounds.min;
  return Math.min(bounds.max, Math.max(bounds.min, Math.round(value)));
}

export default function CoachPage() {
  const toast = useToast();
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [applyBusy, setApplyBusy] = useState<string | null>(null);
  const [checked, setChecked] = useState<Record<string, Record<string, boolean>>>({});
  const [editedPct, setEditedPct] = useState<Record<string, Record<string, number>>>({});
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [turns, busy]);

  const canSend = useMemo(() => draft.trim().length > 0 && !busy, [draft, busy]);

  async function onSend(e?: FormEvent) {
    e?.preventDefault();
    const message = draft.trim();
    if (!message || busy) return;

    const history = turns
      .filter((t) => !t.error)
      .slice(-8)
      .map((t) => {
        const extras =
          t.role === 'assistant' && t.proposal?.clarifyingQuestions?.length
            ? `\n${t.proposal.clarifyingQuestions.map((q) => `- ${q}`).join('\n')}`
            : '';
        const diff =
          t.role === 'assistant' && t.proposal?.diff?.length
            ? `\nProposed: ${t.proposal.diff.map((r) => `${r.label} ${r.currentPct}% → ${r.proposedPct}%`).join('; ')}`
            : '';
        return { role: t.role, content: `${t.text}${extras}${diff}` };
      });

    const userTurn: ChatTurn = { id: uid(), role: 'user', text: message };
    setTurns((prev) => [...prev, userTurn]);
    setDraft('');
    setBusy(true);

    try {
      const proposal = await api<CoachProposal>('strategy_coach_propose', {
        message,
        history,
      });
      const assistant: ChatTurn = {
        id: uid(),
        role: 'assistant',
        text: proposal.summary,
        proposal,
      };
      if (proposal.diff?.length) {
        setChecked((prev) => ({
          ...prev,
          [assistant.id]: Object.fromEntries(proposal.diff.map((row) => [row.field, true])),
        }));
        setEditedPct((prev) => ({
          ...prev,
          [assistant.id]: Object.fromEntries(
            proposal.diff.map((row) => [row.field, clampPct(row.field, row.proposedPct)]),
          ),
        }));
      }
      setTurns((prev) => [...prev, assistant]);
    } catch (err) {
      const text = String(err);
      toast.error(text, 'Coach');
      setTurns((prev) => [
        ...prev,
        {
          id: uid(),
          role: 'assistant',
          text,
          error: true,
        },
      ]);
    } finally {
      setBusy(false);
    }
  }

  async function onApply(turn: ChatTurn) {
    const proposal = turn.proposal;
    if (!proposal || turn.applied || turn.rejected) return;
    const selectedFlags = checked[turn.id] || {};
    const selected: Record<string, number> = {};
    for (const row of proposal.diff) {
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
        summary: proposal.summary,
        chatExcerpt: excerptFromTurns(turns, turn.text),
      });
      setTurns((prev) =>
        prev.map((t) => (t.id === turn.id ? { ...t, applied: true } : t)),
      );
      toast.success(
        'Strategy parameters were saved. Settings sliders now show the new values.',
        'Coach',
      );
    } catch (err) {
      toast.error(String(err), 'Coach');
    } finally {
      setApplyBusy(null);
    }
  }

  function onReject(turn: ChatTurn) {
    setTurns((prev) =>
      prev.map((t) => (t.id === turn.id ? { ...t, rejected: true } : t)),
    );
  }

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Coach</h1>
          <p>
            Talk through risk and sizing in plain language. Slider changes apply only after you
            confirm — Settings remains the source of truth. This chat does not place trades.
          </p>
        </div>
      </header>

      <div className="panel coach-panel">
        <div className="coach-log" role="log" aria-live="polite">
          {turns.length === 0 && !busy ? (
            <div className="empty-state">
              <div className="empty-state__icon">
                <IconStrategy size={22} />
              </div>
              <div>Ask the coach about your strategy</div>
              <p className="muted" style={{ margin: 0 }}>
                Examples: tighten risk after losses, deploy idle cash, lock gains sooner, or reset to
                balanced defaults for a ₦ account.
              </p>
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
              <p className="coach-bubble__text">{turn.text}</p>

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
                    <li key={q}>{q}</li>
                  ))}
                </ul>
              ) : null}

              {turn.proposal && turn.proposal.diff.length > 0 ? (
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
                      {turn.proposal.diff.map((row) => (
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
                                min={FIELD_PCT_BOUNDS[row.field]?.min ?? 1}
                                max={FIELD_PCT_BOUNDS[row.field]?.max ?? 100}
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
                  {turn.applied ? (
                    <p className="muted" style={{ marginBottom: 0 }}>
                      Saved to Settings. Live trading rules are unchanged until the next cycle uses
                      these sliders.
                    </p>
                  ) : null}
                </div>
              ) : null}
            </article>
          ))}
          {busy ? (
            <article className="coach-bubble coach-bubble--assistant">
              <div className="coach-bubble__role">Coach</div>
              <p className="muted" style={{ margin: 0 }}>Thinking…</p>
            </article>
          ) : null}
          <div ref={bottomRef} />
        </div>

        <form className="coach-composer" onSubmit={(e) => void onSend(e)}>
          <input
            className="input"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="Ask about your strategy, or describe a change — e.g. tighten risk after this week’s losses"
            aria-label="Message the strategy coach"
            disabled={busy}
          />
          <button className="btn btn-primary" type="submit" disabled={!canSend}>
            {busy ? 'Sending…' : 'Send'}
          </button>
        </form>
      </div>
    </div>
  );
}
