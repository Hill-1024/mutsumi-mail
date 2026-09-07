import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { transitionView } from './navigation';

beforeEach(() => {
  vi.stubGlobal('matchMedia', vi.fn().mockReturnValue({ matches: false }));
});
afterEach(() => {
  Reflect.deleteProperty(document, 'startViewTransition');
  delete document.documentElement.dataset.viewTransition;
  vi.unstubAllGlobals();
});
it('ignores an older deferred navigation when a newer one supersedes it', async () => {
  const callbacks: Array<() => void> = [];
  const finish: Array<() => void> = [];
  const skip = vi.fn();
  Object.defineProperty(document, 'startViewTransition', { configurable: true, value: (update: () => void) => {
    callbacks.push(update);
    return { skipTransition: skip, finished: new Promise<void>(resolve => finish.push(resolve)) };
  } });
  const first = vi.fn(); const second = vi.fn();
  transitionView(first); transitionView(second);
  callbacks[0](); callbacks[1]();
  expect(first).not.toHaveBeenCalled(); expect(second).toHaveBeenCalledOnce(); expect(skip).toHaveBeenCalledOnce();
  finish[0](); await Promise.resolve(); await Promise.resolve();
  expect(document.documentElement.dataset.viewTransition).toBe('active');
  finish[1](); await Promise.resolve(); await Promise.resolve();
  expect(document.documentElement.dataset.viewTransition).toBeUndefined();
});
it('uses an immediate atomic update when reduced motion is requested', () => {
  vi.mocked(window.matchMedia).mockReturnValue({ matches: true } as MediaQueryList);
  const start = vi.fn();
  Object.defineProperty(document, 'startViewTransition', { configurable: true, value: start });
  const update = vi.fn(); transitionView(update);
  expect(update).toHaveBeenCalledOnce(); expect(start).not.toHaveBeenCalled();
});
it('still navigates when the native transition API throws', () => {
  Object.defineProperty(document, 'startViewTransition', { configurable: true, value: () => { throw new Error('unavailable'); } });
  const update = vi.fn(); transitionView(update);
  expect(update).toHaveBeenCalledOnce();
  expect(document.documentElement.dataset.viewTransition).toBeUndefined();
});
