import { QueryClient } from '@tanstack/react-query';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { applyFlagMutation, readMailData } from './optimistic-flags';
import { mutateMessage, mutateMessages } from './tauri';
import type { Mailbox, Message } from '../types';

vi.mock('./tauri', () => ({ mutateMessage: vi.fn(), mutateMessages: vi.fn() }));
const message: Message = {
  id: 'm', accountId: 'a', mailboxId: 'inbox-a', threadId: 't', subject: 'Subject', normalizedSubject: 'Subject',
  from: { email: 'sender@example.com' }, to: [], date: '', preview: '', isRead: false,
  isStarred: false, hasAttachment: false, labels: [],
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const mailbox: Mailbox = { id: 'inbox-a', accountId: 'a', remoteId: 'INBOX', name: 'INBOX', displayName: 'Inbox', unreadCount: 1, totalCount: 1, syncEnabled: true };
  for (const key of [['messages', 'all', 'inbox'], ['messages', 'a', 'inbox-a'], ['search', 'all', 'Subject']]) {
    client.setQueryData(key, [message, { ...message, mailboxId: 'archive-a' }]);
  }
  for (const scope of ['all', 'a']) client.setQueryData(['mailboxes', scope], [mailbox]);
  const row = () => client.getQueryData<Message[]>(['messages', 'all', 'inbox'])![0];
  const count = () => client.getQueryData<Mailbox[]>(['mailboxes', 'all'])![0].unreadCount;
  return { client, row, count };
}

beforeEach(() => vi.resetAllMocks());

describe('mail flag transactions', () => {
  it('updates rows, search results and all scoped counters together, without touching another instance', async () => {
    const { client, row, count } = setup();
    const call = deferred<Message>();
    vi.mocked(mutateMessage).mockReturnValue(call.promise);
    const task = applyFlagMutation(client, [row()], { isRead: true });
    expect(row().isRead).toBe(true);
    expect(count()).toBe(0);
    expect(client.getQueryData<Message[]>(['search', 'all', 'Subject'])!.map(m => m.isRead)).toEqual([true, false]);
    expect(client.getQueryData<Mailbox[]>(['mailboxes', 'a'])![0].unreadCount).toBe(0);
    call.resolve({ ...message, isRead: true });
    await task;
    expect(row().isRead).toBe(true);
    expect(count()).toBe(0);
  });

  it('serializes auto-read and repeated true -> false -> true clicks without an ABA rollback', async () => {
    const { client, row, count } = setup();
    const calls = [deferred<Message>(), deferred<Message>(), deferred<Message>()];
    calls.forEach(call => vi.mocked(mutateMessage).mockReturnValueOnce(call.promise));
    const first = applyFlagMutation(client, [row()], { isRead: true });
    const second = applyFlagMutation(client, [row()], { isRead: false });
    const third = applyFlagMutation(client, [row()], { isRead: true });
    await vi.waitFor(() => expect(mutateMessage).toHaveBeenCalledTimes(1));
    calls[0].resolve({ ...message, isRead: true }); await first;
    await vi.waitFor(() => expect(mutateMessage).toHaveBeenCalledTimes(2));
    expect(row().isRead).toBe(true); expect(count()).toBe(0);
    calls[1].resolve({ ...message, isRead: false }); await second;
    expect(row().isRead).toBe(true); expect(count()).toBe(0);
    calls[2].resolve({ ...message, isRead: true }); await third;
    expect(row().isRead).toBe(true); expect(count()).toBe(0);
  });

  it('rolls back failed writes to the last committed state, not another failed optimistic snapshot', async () => {
    const { client, row, count } = setup();
    const calls = [deferred<Message>(), deferred<Message>()];
    calls.forEach(call => vi.mocked(mutateMessage).mockReturnValueOnce(call.promise));
    const first = applyFlagMutation(client, [row()], { isRead: true }).catch(() => undefined);
    const second = applyFlagMutation(client, [row()], { isRead: false, isStarred: true }).catch(() => undefined);
    calls[0].reject(new Error('first')); await first;
    expect(row().isStarred).toBe(true); expect(count()).toBe(1);
    calls[1].reject(new Error('second')); await second;
    expect(row()).toMatchObject({ isRead: false, isStarred: false }); expect(count()).toBe(1);
  });

  it('cancels a stale in-flight read and delays a new route query until commits finish', async () => {
    const { client, row, count } = setup();
    const stale = deferred<Message[]>();
    const staleQuery = client.fetchQuery({ queryKey: ['messages', 'all', 'inbox'], queryFn: () => stale.promise }).catch(() => undefined);
    const call = deferred<Message>(); vi.mocked(mutateMessage).mockReturnValue(call.promise);
    const task = applyFlagMutation(client, [row()], { isRead: true });
    const read = vi.fn().mockResolvedValue([{ ...message, isRead: true }]);
    const newRoute = readMailData(client, read);
    stale.resolve([message]); await staleQuery;
    expect(row().isRead).toBe(true); expect(count()).toBe(0); expect(read).not.toHaveBeenCalled();
    call.resolve({ ...message, isRead: true }); await task; await newRoute;
    expect(read).toHaveBeenCalledOnce();
  });

  it('deduplicates bulk instances and rolls back both rows and counts on a partial acknowledgement', async () => {
    const { client, row, count } = setup();
    vi.mocked(mutateMessages).mockResolvedValue({ mutated: 1 });
    await expect(applyFlagMutation(client, [row(), row(), { ...message, id: 'second' }], { isRead: true })).rejects.toThrow('部分');
    expect(vi.mocked(mutateMessages).mock.calls[0][0]).toHaveLength(2);
    expect(row().isRead).toBe(false);
    expect(count()).toBe(1);
  });

  it('rejects mismatched result identities instead of accepting a false success', async () => {
    const { client, row, count } = setup();
    vi.mocked(mutateMessage).mockResolvedValue({ ...message, isRead: true, mailboxId: 'wrong' });
    await expect(applyFlagMutation(client, [row()], { isRead: true })).rejects.toThrow('不一致');
    expect(row().isRead).toBe(false); expect(count()).toBe(1);
  });
});
