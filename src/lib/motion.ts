import { useEffect, useRef, useState } from 'react';

export interface ExitPresence {
  mounted: boolean;
  /** True while the element should render with its `is-exiting` class. */
  exiting: boolean;
  /**
   * Attach to the element that plays the exit animation. The unmount happens when
   * the animation actually finishes; the target check ignores bubbled events from
   * animated children.
   */
  handleAnimationEnd: (event: {
    target: EventTarget | null;
    currentTarget: Element;
  }) => void;
}

type PresencePhase = 'closed' | 'open' | 'exiting';

/**
 * Keeps a component mounted while its CSS exit animation plays.
 *
 * A single phase variable drives everything, and the render-time transition below
 * re-evaluates on every render, so a dropped render pass can never desync the state:
 * the phase self-heals on the next render. The unmount is driven by `animationend`
 * on the exiting element, with a guarded timeout as a fallback for elements whose
 * animation may never run (e.g. hidden behind a media query). The fallback checks
 * `open` through a ref at fire time, so the worst case is a skipped exit animation —
 * never a vanishing open surface.
 */
export function useExitPresence(open: boolean, exitMs: number): ExitPresence {
  const [phase, setPhase] = useState<PresencePhase>(open ? 'open' : 'closed');
  if (open && phase !== 'open') {
    setPhase('open');
  } else if (!open && phase === 'open') {
    setPhase('exiting');
  }
  const openRef = useRef(open);
  useEffect(() => {
    // Runs before the fallback timer can ever fire; keeps the fire-time guard honest.
    openRef.current = open;
  }, [open]);
  useEffect(() => {
    if (open) return undefined;
    const timer = window.setTimeout(() => {
      if (!openRef.current) setPhase('closed');
    }, exitMs + 250);
    return () => window.clearTimeout(timer);
  }, [exitMs, open]);
  return {
    mounted: phase !== 'closed',
    exiting: phase === 'exiting',
    handleAnimationEnd: (event) => {
      if (event.target === event.currentTarget && !openRef.current) setPhase('closed');
    },
  };
}

/**
 * Returns a counter that increments whenever `value` changes after mount.
 *
 * Keying an icon by the counter replays its entrance animation on real state changes
 * only — virtualized rows remount while scrolling, and a plain CSS class change would
 * replay the pop for already-selected rows every time they scroll back into view.
 */
export function useChangePulse<T>(value: T): number {
  const [pulse, setPulse] = useState(0);
  const [prevValue, setPrevValue] = useState(value);
  if (!Object.is(prevValue, value)) {
    setPrevValue(value);
    setPulse((current) => current + 1);
  }
  return pulse;
}
