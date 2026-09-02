import type { Message } from '../types';

export function normalizeSubject(subject: string): string {
  return subject.replace(/^(?:(?:re|fw|fwd)\s*:\s*)+/giu, '').replace(/\s+/g, ' ').trim().toLocaleLowerCase();
}

export interface SearchFilters {
  freeText: string;
  from?: string;
  to?: string;
  subject?: string;
  isUnread?: boolean;
  isStarred?: boolean;
  hasAttachment?: boolean;
  before?: string;
  after?: string;
  account?: string;
  folder?: string;
}

export function parseSearch(input: string): SearchFilters {
  const tokens = input.match(/(?:[^\s"]+|"[^"]*")+/g) ?? [];
  const filters: SearchFilters = { freeText: '' };
  const free: string[] = [];
  for (const token of tokens) {
    const separator = token.indexOf(':');
    if (separator <= 0) { free.push(token); continue; }
    const key = token.slice(0, separator).toLowerCase();
    const raw = token.slice(separator + 1).replace(/^"|"$/g, '');
    if (key === 'from' || key === 'to' || key === 'cc' || key === 'subject' || key === 'before' || key === 'after' || key === 'account' || key === 'folder') (filters as unknown as Record<string, unknown>)[key] = raw;
    else if (key === 'is' && raw === 'unread') filters.isUnread = true;
    else if (key === 'is' && raw === 'starred') filters.isStarred = true;
    else if (key === 'has' && raw === 'attachment') filters.hasAttachment = true;
    else free.push(token);
  }
  filters.freeText = free.join(' ').trim();
  return filters;
}

export function filterMessages(messages: Message[], input: string): Message[] {
  const filters = parseSearch(input);
  const q = filters.freeText.toLocaleLowerCase();
  return messages.filter((message) => {
    const haystack = `${message.subject} ${message.preview} ${message.bodyText ?? ''} ${message.from.email}`.toLocaleLowerCase();
    return (!q || haystack.includes(q)) && (!filters.from || message.from.email.toLocaleLowerCase().includes(filters.from.toLocaleLowerCase())) && (!filters.subject || message.subject.toLocaleLowerCase().includes(filters.subject.toLocaleLowerCase())) && (!filters.isUnread || !message.isRead) && (!filters.isStarred || message.isStarred) && (!filters.hasAttachment || message.hasAttachment) && (!filters.before || message.date.slice(0, 10) < filters.before) && (!filters.after || message.date.slice(0, 10) > filters.after);
  });
}

export function safeHtmlToText(html: string): string {
  if (typeof DOMParser === 'undefined') return html.replace(/<[^>]*>/g, ' ');
  const doc = new DOMParser().parseFromString(html, 'text/html');
  doc.querySelectorAll('script,style,iframe,object,embed,form').forEach((node) => node.remove());
  return (doc.body.textContent ?? '').replace(/\s+/g, ' ').trim();
}
