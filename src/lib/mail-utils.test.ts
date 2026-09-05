import { describe, expect, it } from 'vitest';
import {
  filterMessages,
  normalizeSubject,
  parseSearch,
  safeHtmlToText,
  buildEmailDocument,
} from './mail-utils';
import type { Message } from '../types';

const message = (id: string, flags: Pick<Message, 'isRead' | 'isStarred' | 'hasAttachment'>): Message => ({
  id,
  accountId: 'account-test',
  mailboxId: 'mailbox-test',
  threadId: `thread-${id}`,
  subject: `Message ${id}`,
  normalizedSubject: `message ${id}`,
  from: { email: `${id}@example.test` },
  to: [],
  date: '2026-01-01T00:00:00Z',
  preview: '',
  labels: [],
  ...flags,
});

const messages = [
  message('one', { isRead: false, isStarred: true, hasAttachment: true }),
  message('two', { isRead: false, isStarred: false, hasAttachment: true }),
  message('three', { isRead: true, isStarred: true, hasAttachment: true }),
];

describe('mail utilities', () => {
  it('normalizes reply prefixes without collapsing meaningful text', () => {
    expect(normalizeSubject('Re: Fwd:  Project update')).toBe('project update');
  });

  it('parses structured cached search filters', () => {
    expect(parseSearch('from:takeshi@example.com is:unread has:attachment review')).toMatchObject({ from: 'takeshi@example.com', isUnread: true, hasAttachment: true, freeText: 'review' });
  });

  it('filters messages locally', () => {
    expect(filterMessages(messages, 'is:unread')).toHaveLength(2);
    expect(filterMessages(messages, 'has:attachment')).toHaveLength(3);
    expect(filterMessages(messages, 'is:starred')).toHaveLength(2);
  });

  it('extracts text while removing active HTML elements', () => {
    expect(safeHtmlToText('<p>Hello</p><script>steal()</script><form>bad</form>')).toBe('Hello');
  });

  it('preserves the complete authored HTML document including styles and images', () => {
    const original = '<html><head><style>.hero{display:grid;background:#ffc}</style></head><body><div class="hero"><img src="https://example.com/logo.png">正文</div></body></html>';
    expect(buildEmailDocument(original)).toContain(original);
  });
});

it('applies recipient, account and folder filters instead of silently ignoring them', () => {
  const item = { ...messages[0], to: [{ email: 'person@example.com' }], labels: ['收件箱'] };
  expect(filterMessages([item], 'to:person account:account-test folder:收件箱')).toHaveLength(1);
  expect(filterMessages([item], 'to:other')).toHaveLength(0);
  expect(filterMessages([item], 'account:other')).toHaveLength(0);
  expect(filterMessages([item], 'folder:other')).toHaveLength(0);
});
