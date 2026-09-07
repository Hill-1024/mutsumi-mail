import { type QueryClient } from '@tanstack/react-query';
import type { Mailbox, Message } from '../types';
import { mutateMessage, mutateMessages } from './tauri';

export interface FlagMutation {
  isRead?: boolean;
  isStarred?: boolean;
}

export const messageInstanceKey = (message: Pick<Message, 'id' | 'mailboxId'>) =>
  `${message.id}\u0000${message.mailboxId}`;

type Flags = Pick<Message, 'isRead' | 'isStarred'>;
interface PendingFlags {
  confirmed: Flags;
  intents: Map<symbol, FlagMutation>;
}
interface FlagQueue {
  tail: Promise<void>;
  pending: number;
  records: Map<string, PendingFlags>;
}

// Owned by the cache, not a route: navigation cannot lose pending writes.
const queues = new WeakMap<QueryClient, FlagQueue>();
function queueFor(client: QueryClient): FlagQueue {
  let queue = queues.get(client);
  if (!queue) {
    queue = { tail: Promise.resolve(), pending: 0, records: new Map() };
    queues.set(client, queue);
  }
  return queue;
}

const mailQueryKeys = ['messages', 'message', 'search', 'mailboxes'];

/** New routes and background refetches must read after pending local writes. */
export async function readMailData<T>(client: QueryClient, read: () => Promise<T>): Promise<T> {
  const queue = queueFor(client);
  while (queue.pending) await queue.tail;
  return read();
}

function visibleFlags(record: PendingFlags): Flags {
  return Object.assign({}, record.confirmed, ...record.intents.values());
}

function writeFlags(client: QueryClient, source: Message, before: Flags, after: Flags) {
  const key = messageInstanceKey(source);
  for (const queryKey of ['messages', 'search']) {
    client.setQueriesData<Message[]>({ queryKey: [queryKey] }, (items) =>
      items?.map((item) => messageInstanceKey(item) === key ? { ...item, ...after } : item),
    );
  }
  client.setQueriesData<Message>({ queryKey: ['message'] }, (item) =>
    item && messageInstanceKey(item) === key ? { ...item, ...after } : item,
  );
  const unreadDelta = Number(before.isRead) - Number(after.isRead);
  if (unreadDelta) {
    client.setQueriesData<Mailbox[]>({ queryKey: ['mailboxes'] }, (items) =>
      items?.map((item) => item.id === source.mailboxId && item.accountId === source.accountId
        ? { ...item, unreadCount: Math.max(0, Math.min(item.totalCount, item.unreadCount + unreadDelta)) }
        : item),
    );
  }
}

export function applyFlagMutation(
  client: QueryClient,
  messages: Message[],
  mutation: FlagMutation,
): Promise<void> {
  const unique = [...new Map(messages.map((message) => [messageInstanceKey(message), message])).values()];
  if (!unique.length || (mutation.isRead === undefined && mutation.isStarred === undefined)) {
    return Promise.resolve();
  }
  const queue = queueFor(client);
  const operation = Symbol('flag mutation');
  queue.pending += 1;
  // cancelQueries initiates cancellation synchronously, before cache changes.
  const cancelled = Promise.all(mailQueryKeys.map((key) => client.cancelQueries({ queryKey: [key] })));
  for (const source of unique) {
    const key = messageInstanceKey(source);
    const cached = client.getQueriesData<Message[]>({ queryKey: ['messages'] })
      .flatMap(([, items]) => items ?? []).find((item) => messageInstanceKey(item) === key) ?? source;
    const record = queue.records.get(key) ?? {
      confirmed: { isRead: cached.isRead, isStarred: cached.isStarred }, intents: new Map(),
    };
    const before = visibleFlags(record);
    record.intents.set(operation, mutation);
    queue.records.set(key, record);
    writeFlags(client, source, before, visibleFlags(record));
  }

  const task = queue.tail.then(async () => {
    let committed = false;
    try {
      await cancelled;
      const refs = unique.map((message) => ({ messageId: message.id, mailboxId: message.mailboxId }));
      if (refs.length === 1) {
        const result = await mutateMessage(refs[0], mutation);
        if (result.id !== refs[0].messageId || result.mailboxId !== refs[0].mailboxId
          || (mutation.isRead !== undefined && result.isRead !== mutation.isRead)
          || (mutation.isStarred !== undefined && result.isStarred !== mutation.isStarred)) {
          throw new Error('邮件状态更新返回了不一致的结果');
        }
      } else {
        const result = await mutateMessages(refs, mutation);
        if (result.mutated !== refs.length) throw new Error('部分邮件状态未能更新');
      }
      committed = true;
    } finally {
      for (const source of unique) {
        const key = messageInstanceKey(source);
        const record = queue.records.get(key);
        if (!record) continue;
        const before = visibleFlags(record);
        if (committed) Object.assign(record.confirmed, mutation);
        // Operation identity handles true -> false -> true; value equality cannot.
        record.intents.delete(operation);
        writeFlags(client, source, before, visibleFlags(record));
        if (!record.intents.size) queue.records.delete(key);
      }
      queue.pending -= 1;
      if (!queue.pending) {
        // Cache values are already committed. Refetch updates folder membership.
        for (const key of mailQueryKeys) void client.invalidateQueries({ queryKey: [key] });
      }
    }
  });
  queue.tail = task.catch(() => undefined);
  return task;
}
