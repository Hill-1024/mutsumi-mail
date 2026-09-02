import { describe, expect, it } from 'vitest';
import { filterMessages, normalizeSubject, parseSearch, safeHtmlToText } from './mail-utils';
import { sampleMessages } from '../data/sample';

describe('mail utilities', () => {
  it('normalizes reply prefixes without collapsing meaningful text', () => {
    expect(normalizeSubject('Re: Fwd:  Q4 design review')).toBe('q4 design review');
  });

  it('parses structured cached search filters', () => {
    expect(parseSearch('from:takeshi@example.com is:unread has:attachment review')).toMatchObject({ from: 'takeshi@example.com', isUnread: true, hasAttachment: true, freeText: 'review' });
  });

  it('filters messages locally', () => {
    expect(filterMessages(sampleMessages, 'is:unread')).toHaveLength(2);
    expect(filterMessages(sampleMessages, 'has:attachment')).toHaveLength(3);
  });

  it('extracts text while removing active HTML elements', () => {
    expect(safeHtmlToText('<p>Hello</p><script>steal()</script><form>bad</form>')).toBe('Hello');
  });
});
