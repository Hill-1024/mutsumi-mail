import { act, render } from '@testing-library/react';
import { StrictMode, useEffect } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useExitPresence } from './motion';

function Probe({
  open,
  onState,
}: {
  open: boolean;
  onState: (state: { mounted: boolean; exiting: boolean }) => void;
}) {
  const presence = useExitPresence(open, 180);
  useEffect(() => {
    onState({ mounted: presence.mounted, exiting: presence.exiting });
  }, [onState, presence.exiting, presence.mounted]);
  return (
    <div
      className={presence.exiting ? 'is-exiting' : ''}
      onAnimationEnd={presence.handleAnimationEnd}
    />
  );
}

function renderProbe(ui: React.ReactElement, strict: boolean) {
  return strict ? render(<StrictMode>{ui}</StrictMode>) : render(ui);
}

describe('useExitPresence', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it.each([false, true])(
    'mounts when opened and stays mounted while open, even past the fallback (strict=%s)',
    (strict) => {
      const states: Array<{ mounted: boolean; exiting: boolean }> = [];
      const { rerender } = renderProbe(
        <Probe open={false} onState={(s) => states.push(s)} />,
        strict,
      );
      rerender(
        strict ? (
          <StrictMode>
            <Probe open onState={(s) => states.push(s)} />
          </StrictMode>
        ) : (
          <Probe open onState={(s) => states.push(s)} />
        ),
      );
      expect(states.at(-1)).toEqual({ mounted: true, exiting: false });
      act(() => {
        vi.advanceTimersByTime(2000);
      });
      // Open the whole time: must remain mounted far beyond the fallback delay.
      expect(states.at(-1)).toEqual({ mounted: true, exiting: false });
    },
  );

  it.each([false, true])(
    'unmounts via animationend after closing and never via a stray timeout while open (strict=%s)',
    (strict) => {
      const states: Array<{ mounted: boolean; exiting: boolean }> = [];
      const { rerender, container } = renderProbe(
        <Probe open onState={(s) => states.push(s)} />,
        strict,
      );
      rerender(
        strict ? (
          <StrictMode>
            <Probe open={false} onState={(s) => states.push(s)} />
          </StrictMode>
        ) : (
          <Probe open={false} onState={(s) => states.push(s)} />
        ),
      );
      // Still mounted with the exiting class during the animation window.
      expect(states.at(-1)).toEqual({ mounted: true, exiting: true });
      expect(container.querySelector('.is-exiting')).not.toBeNull();

      // The exit animation ends on the scrim element itself.
      const exiting = container.querySelector('.is-exiting')!;
      act(() => {
        exiting.dispatchEvent(
          new Event('animationend', { bubbles: true }),
        );
      });
      expect(states.at(-1)).toEqual({ mounted: false, exiting: false });
    },
  );

  it('ignores bubbled animationend from children and releases via the fallback timer', () => {
    const states: Array<{ mounted: boolean; exiting: boolean }> = [];
    const { rerender, container } = renderProbe(
      <Probe open onState={(s) => states.push(s)} />,
      false,
    );
    rerender(<Probe open={false} onState={(s) => states.push(s)} />);
    const child = document.createElement('div');
    container.querySelector('.is-exiting')!.appendChild(child);
    act(() => {
      child.dispatchEvent(new Event('animationend', { bubbles: true }));
    });
    // Bubbled event must not unmount; the guarded fallback eventually does.
    expect(states.at(-1)?.mounted).toBe(true);
    act(() => {
      vi.advanceTimersByTime(600);
    });
    expect(states.at(-1)).toEqual({ mounted: false, exiting: false });
  });

  it('cancels the pending release when reopened before the animation ends', () => {
    const states: Array<{ mounted: boolean; exiting: boolean }> = [];
    const { rerender } = renderProbe(<Probe open onState={(s) => states.push(s)} />, false);
    rerender(<Probe open={false} onState={(s) => states.push(s)} />);
    rerender(<Probe open onState={(s) => states.push(s)} />);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(states.at(-1)).toEqual({ mounted: true, exiting: false });
  });
});
