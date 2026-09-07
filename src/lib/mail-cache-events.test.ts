import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { subscribeToMailCache } from './mail-cache-events';

const native = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));

describe('mail cache commit events', () => {
  beforeEach(() => { vi.useFakeTimers(); native.listen.mockReset(); });
  afterEach(() => { vi.useRealTimers(); });

  it('displays a committed arrival before the account finishes syncing', async () => {
    let onCommit = () => {};
    native.listen.mockImplementation(async (event: string, callback: () => void) => {
      expect(event).toBe('mail-cache-changed');
      onCommit = callback;
      return () => {};
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let stored = ['old mail'];
    const observer = new QueryObserver(client, {
      queryKey: ['messages', 'all', 'inbox'], queryFn: async () => [...stored],
    });
    const stopObserver = observer.subscribe(() => {});
    await observer.refetch();
    const dispose = subscribeToMailCache(client);
    await vi.advanceTimersByTimeAsync(100);
    expect(observer.getCurrentResult().data).toEqual(['old mail']);
    stored = ['new mail', 'old mail'];
    onCommit(); // No terminal sync-progress event is emitted.
    await vi.advanceTimersByTimeAsync(100);
    expect(observer.getCurrentResult().data).toEqual(['new mail', 'old mail']);
    dispose(); stopObserver(); client.clear();
  });

  it('coalesces bursts and refreshes counts, lists, details and search together', async () => {
    let onCommit = () => {};
    const unlisten = vi.fn();
    native.listen.mockImplementation(async (_event: string, callback: () => void) => {
      onCommit = callback;
      return unlisten;
    });
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    const dispose = subscribeToMailCache(client);
    await vi.advanceTimersByTimeAsync(100);
    invalidate.mockClear();
    onCommit();
    await vi.advanceTimersByTimeAsync(90);
    onCommit(); onCommit();
    await vi.advanceTimersByTimeAsync(10);
    expect(invalidate.mock.calls.map(([filters]) => filters?.queryKey)).toEqual([
      ['mailboxes'], ['messages'], ['message'], ['search'],
    ]);
    onCommit();
    dispose();
    await vi.advanceTimersByTimeAsync(100);
    expect(invalidate).toHaveBeenCalledTimes(4);
    expect(unlisten).toHaveBeenCalledOnce();
    client.clear();
  });

  it('disposes a listener that finishes registering after unmount', async () => {
    let registered!: (dispose: () => void) => void;
    native.listen.mockReturnValue(new Promise<() => void>((resolve) => { registered = resolve; }));
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    const dispose = subscribeToMailCache(client);
    dispose();
    const unlisten = vi.fn();
    registered(unlisten);
    await vi.advanceTimersByTimeAsync(100);
    expect(unlisten).toHaveBeenCalledOnce();
    expect(invalidate).not.toHaveBeenCalled();
    client.clear();
  });
});
