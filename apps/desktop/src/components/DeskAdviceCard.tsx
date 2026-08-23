import { Link } from 'react-router-dom';
import type { DeskAdvice } from '../lib/api';

type Props = {
  advice: DeskAdvice | null;
  compact?: boolean;
};

export function DeskAdviceCard({ advice, compact }: Props) {
  if (!advice?.applicable) return null;

  return (
    <section
      className={compact ? 'desk-advice desk-advice--embedded' : 'desk-advice panel panel--danger'}
      aria-labelledby="desk-advice-heading"
    >
      <div className="desk-advice__meta">
        <span className="status-pill status-pill--warn">Advice</span>
        <span className="muted">Nothing was applied</span>
      </div>
      <h2 id="desk-advice-heading">{advice.title}</h2>
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
      {!compact ? (
        <ul className="desk-advice__donot">
          {advice.doNot.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      ) : null}
      {compact ? (
        <p className="muted" style={{ marginBottom: 0 }}>
          Apply these yourself on the sliders below. Coach can explain them — it cannot save them.
        </p>
      ) : (
        <p className="desk-advice__cta">
          <Link to="/settings#strategy">Open Settings to apply</Link>
          {' · '}
          <Link to="/coach">Ask Coach</Link>
        </p>
      )}
    </section>
  );
}
