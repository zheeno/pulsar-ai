import { useEffect, useMemo, useRef, useState } from 'react';
import pulsarMark from '../assets/pulsar-logo.svg';
import pulsarFull from '../assets/pulsar-logo-full.svg';
import { hideNativeSplash } from '../lib/native-splash';

type Props = {
  ready: boolean;
  message?: string;
  onFinished: () => void;
};

type StarSpec = {
  top: string;
  left: string;
  size: number;
  delay: string;
  duration: string;
  kind: 'cross' | 'dot' | 'pulse';
};

const STARS: StarSpec[] = [
  { top: '12%', left: '14%', size: 10, delay: '0s', duration: '2.8s', kind: 'cross' },
  { top: '18%', left: '78%', size: 14, delay: '0.4s', duration: '3.2s', kind: 'pulse' },
  { top: '28%', left: '88%', size: 8, delay: '1.1s', duration: '2.4s', kind: 'dot' },
  { top: '36%', left: '8%', size: 12, delay: '0.7s', duration: '3.6s', kind: 'pulse' },
  { top: '48%', left: '92%', size: 9, delay: '1.6s', duration: '2.9s', kind: 'cross' },
  { top: '62%', left: '11%', size: 7, delay: '0.2s', duration: '2.2s', kind: 'dot' },
  { top: '68%', left: '84%', size: 13, delay: '0.9s', duration: '3.4s', kind: 'pulse' },
  { top: '76%', left: '22%', size: 10, delay: '1.3s', duration: '2.7s', kind: 'cross' },
  { top: '82%', left: '70%', size: 8, delay: '0.5s', duration: '3.1s', kind: 'dot' },
  { top: '22%', left: '42%', size: 6, delay: '1.8s', duration: '2.5s', kind: 'dot' },
  { top: '58%', left: '48%', size: 7, delay: '0.35s', duration: '3.8s', kind: 'cross' },
  { top: '8%', left: '55%', size: 11, delay: '1.4s', duration: '3s', kind: 'pulse' },
];

/** Hold mark alone, then cross-fade to wordmark, then allow dismiss once boot is ready. */
const MARK_MS = 2000;
const SEQUENCE_MS = 4800;
const EXIT_MS = 700;

export default function SplashScreen({ ready, message, onFinished }: Props) {
  const [phase, setPhase] = useState<'mark' | 'full' | 'exit'>('mark');
  const [sequenceDone, setSequenceDone] = useState(false);
  const onFinishedRef = useRef(onFinished);
  onFinishedRef.current = onFinished;

  useEffect(() => {
    const toFull = window.setTimeout(() => setPhase('full'), MARK_MS);
    const seq = window.setTimeout(() => setSequenceDone(true), SEQUENCE_MS);
    return () => {
      window.clearTimeout(toFull);
      window.clearTimeout(seq);
    };
  }, []);

  // Wait until this tree has painted so hiding #native-splash does not flash
  // an empty #root (the HTML splash is what macOS 26 users see first).
  useEffect(() => {
    let inner = 0;
    const outer = window.requestAnimationFrame(() => {
      inner = window.requestAnimationFrame(() => hideNativeSplash());
    });
    return () => {
      window.cancelAnimationFrame(outer);
      window.cancelAnimationFrame(inner);
    };
  }, []);

  useEffect(() => {
    if (!ready || !sequenceDone) return;

    let cancelled = false;
    setPhase('exit');
    const done = window.setTimeout(() => {
      if (!cancelled) onFinishedRef.current();
    }, EXIT_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(done);
    };
  }, [ready, sequenceDone]);

  const stars = useMemo(() => STARS, []);

  return (
    <div
      className={`splash-screen${phase === 'exit' ? ' splash-screen--exit' : ''}`}
      role="status"
      aria-live="polite"
      aria-busy={!ready}
    >
      <div className="splash-screen__sky" aria-hidden>
        {stars.map((star, i) => (
          <span
            key={i}
            className={`splash-star splash-star--${star.kind}`}
            style={{
              top: star.top,
              left: star.left,
              width: star.size,
              height: star.size,
              animationDelay: star.delay,
              animationDuration: star.duration,
            }}
          />
        ))}
      </div>

      <div className="splash-screen__stage">
        <img
          src={pulsarMark}
          alt=""
          className={`splash-logo splash-logo--mark${phase === 'mark' ? ' is-visible' : ''}`}
        />
        <img
          src={pulsarFull}
          alt="Pulsar AI"
          className={`splash-logo splash-logo--full${phase !== 'mark' ? ' is-visible' : ''}`}
        />
      </div>

      {message ? (
        <p className={`splash-screen__message${phase !== 'mark' ? ' is-visible' : ''}`}>
          {message}
        </p>
      ) : null}
    </div>
  );
}
