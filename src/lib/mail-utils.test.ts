import { describe, expect, it } from 'vitest';
import {
  filterMessages,
  normalizeSubject,
  parseSearch,
  safeHtmlToText,
  sanitizeHtmlForDisplay,
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

  it('保留安全邮件排版并移除脚本、事件和远程图片', () => {
    const html = sanitizeHtmlForDisplay(
      '<table><tr><td style="color:red">Hello</td></tr></table><script>bad()</script><img src="https://tracker.example/pixel" onerror="bad()">',
      true,
    );
    expect(html).toContain('<table>');
    expect(html).toContain('color:red');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('onerror');
    expect(html).not.toContain('tracker.example');
  });
});

it('blocks protocol-relative trackers, relative requests, escaped CSS and unsafe link schemes', () => {
  const html = sanitizeHtmlForDisplay('<img src="//tracker.example/pixel"><img src="/private"><a href="java&#10;script:alert(1)">bad</a><a href="data:text/html,test">data</a><p style="background-image:u\\72l(https://tracker.example);color:red;position:fixed">body</p>', true);
  const doc = new DOMParser().parseFromString(html, 'text/html');
  expect(doc.querySelectorAll('[src],a[href]').length).toBe(0);
  expect(doc.querySelector('p')?.getAttribute('style')).toBe('color:red');
  expect(sanitizeHtmlForDisplay('<img src="https://example.com/image.png">', false)).toContain('src="https://example.com/image.png"');
});

it('applies recipient, account and folder filters instead of silently ignoring them', () => {
  const item = { ...messages[0], to: [{ email: 'person@example.com' }], labels: ['收件箱'] };
  expect(filterMessages([item], 'to:person account:account-test folder:收件箱')).toHaveLength(1);
  expect(filterMessages([item], 'to:other')).toHaveLength(0);
  expect(filterMessages([item], 'account:other')).toHaveLength(0);
  expect(filterMessages([item], 'folder:other')).toHaveLength(0);
});
