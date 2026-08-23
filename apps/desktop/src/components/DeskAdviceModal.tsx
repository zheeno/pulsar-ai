import { useEffect, useId, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type DeskAdvice } from '../lib/api';
import { useCycle } from '../lib/cycle';
import {
  deskAdviceCycleKey,
  readDeskAdvicePrompt,
  writeDeskAdvicePrompt,
} from '../lib/desk-advice-prompt';

type Props = {
  variant?: 'home' | 'settings';
};

export function DeskAdviceHost({ variant = 'home' }: Props) {
  const titleId = useId();
  const { running } = useCycle();
  const wasRunning = useRef(false);
  const [advice, setAdvice] = useState<DeskAdvice | null>(null);
  const [open, setOpen] = useState(false);
  const [silenced, setSilenced] = useState(false);

  async function refresh() {
    try {
      const next = await api<DeskAdvice>('desk_advice');
      setAdvice(next);
      if (!next?.applicable) {
        setOpen(false);
        setSilenced(false);
        return;
      }
      const cycleId = deskAdviceCycleKey(next.lastCycleId);
      const prompt = readDeskAdvicePrompt(cycleId);
      setSilenced(prompt.silenced);
      if (!prompt.silenced && !prompt.dismissed) {
        setOpen(true);
        writeDeskAdvicePrompt(cycleId, { dismissed: true });
      }
    } catch {
      setAdvice(null);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    if (wasRunning.current && !running) {
      void refresh();
    }
    wasRunning.current = running;
  }, [running]);

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false);
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!advice?.applicable) return null;

  const cycleId = deskAdviceCycleKey(advice.lastCycleId);

  function dismiss() {
    writeDeskAdvicePrompt(cycleId, { dismissed: true });
    setOpen(false);
  }

  function silenceUntilNextCycle() {
    writeDeskAdvicePrompt(cycleId, { dismissed: true, silenced: true });
    setSilenced(true);
    setOpen(false);
  }

  return (
    <>
      <button
        type="button"
        className={`desk-advice-toggle${silenced ? ' is-silenced' : ''}`}
        aria-expanded={open}
        aria-controls={open ? titleId : undefined}
        onClick={() => {
          void refresh();
          setOpen(true);
        }}
      >
        <span className="status-pill status-pill--warn">Advice</span>
        {silenced ? 'Desk advice · silenced until next cycle' : 'Desk advice'}
      </button>

      {open ? (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) dismiss();
          }}
        >
          <div
            className="modal modal--desk-advice"
            role="dialog"
            aria-modal="true"
            aria-labelledby={titleId}
          >
            <div className="modal__header">
              <div>
                <div className="desk-advice__meta">
                  <span className="status-pill status-pill--warn">Advice</span>
                  <span className="muted">Nothing was applied</span>
                </div>
                <h2 id={titleId}>{advice.title}</h2>
              </div>
              <button type="button" className="modal__close" aria-label="Dismiss advice" onClick={dismiss}>
                ×
              </button>
            </div>

            <p className="desk-advice__headline">{advice.headline}</p>
            {advice.reasons.length > 0 ? (
              <ul className="desk-advice__reasons">
                {advice.reasons.map((reason) => (
                  <li key={reason}>{reason}</li>
                ))}
              </ul>
            ) : null}

            <h3>Do this now</h3>
            <ol className="desk-advice__steps">
              {advice.nowSteps.map((step) => (
                <li key={step.id} className={step.aligned ? 'is-aligned' : undefined}>
                  <div className="desk-advice__step-top">
                    <strong>{step.label}</strong>
                    <span className={`desk-advice__badge ${step.aligned ? 'is-on' : 'is-off'}`}>
                      {step.aligned ? 'Already set' : 'Needs you'}
                    </span>
                  </div>
                  <p>{step.detail}</p>
                  {step.current || step.suggested ? (
                    <p className="muted desk-advice__compare">
                      Now {step.current ?? '—'}
                      {step.suggested ? ` · suggested ${step.suggested}` : ''}
                    </p>
                  ) : null}
                </li>
              ))}
            </ol>

            <h3>Then set sliders yourself</h3>
            <ul className="desk-advice__steps">
              {advice.sliderSteps.map((step) => (
                <li key={step.id} className={step.aligned ? 'is-aligned' : undefined}>
                  <div className="desk-advice__step-top">
                    <strong>{step.label}</strong>
                    <span className={`desk-advice__badge ${step.aligned ? 'is-on' : 'is-off'}`}>
                      {step.aligned ? 'In band' : `${step.current ?? '—'} → ${step.suggested ?? ''}`}
                    </span>
                  </div>
                  <p>{step.detail}</p>
                </li>
              ))}
            </ul>

            <p className="desk-advice__kpi">{advice.kpi}</p>
            {variant === 'settings' ? (
              <p className="muted">
                Apply these yourself on the sliders below. Coach can explain them — it cannot save them.
              </p>
            ) : (
              <ul className="desk-advice__donot">
                {advice.doNot.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            )}
            {variant === 'home' ? (
              <p className="desk-advice__cta">
                <Link to="/settings#strategy" onClick={dismiss}>Open Settings to apply</Link>
                {' · '}
                <Link to="/coach" onClick={dismiss}>Ask Coach</Link>
              </p>
            ) : null}

            <div className="btn-row desk-advice__actions">
              <button type="button" className="btn btn-ghost" onClick={dismiss}>
                Dismiss
              </button>
              <button type="button" className="btn btn-secondary" onClick={silenceUntilNextCycle}>
                Silence until next cycle
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
