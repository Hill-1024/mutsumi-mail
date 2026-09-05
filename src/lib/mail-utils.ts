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
    if (key === 'from' || key === 'to' || key === 'subject' || key === 'before' || key === 'after' || key === 'account' || key === 'folder') (filters as unknown as Record<string, unknown>)[key] = raw;
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
    const contains = (value: string, query: string) => value.toLocaleLowerCase().includes(query.toLocaleLowerCase());
    return (!filters.to || message.to.some((address) => contains(`${address.name ?? ''} ${address.email}`, filters.to!))) && (!filters.account || contains(message.accountId, filters.account)) && (!filters.folder || [message.mailboxId, ...message.labels].some((label) => contains(label, filters.folder!))) && (!q || haystack.includes(q)) && (!filters.from || message.from.email.toLocaleLowerCase().includes(filters.from.toLocaleLowerCase())) && (!filters.subject || message.subject.toLocaleLowerCase().includes(filters.subject.toLocaleLowerCase())) && (!filters.isUnread || !message.isRead) && (!filters.isStarred || message.isStarred) && (!filters.hasAttachment || message.hasAttachment) && (!filters.before || message.date.slice(0, 10) < filters.before) && (!filters.after || message.date.slice(0, 10) > filters.after);
  });
}

export function safeHtmlToText(html: string): string {
  if (typeof DOMParser === 'undefined') return html.replace(/<[^>]*>/g, ' ');
  const doc = new DOMParser().parseFromString(html, 'text/html');
  doc.querySelectorAll('script,style,iframe,object,embed,form').forEach((node) => node.remove());
  return (doc.body.textContent ?? '').replace(/\s+/g, ' ').trim();
}

export function sanitizeHtmlForDisplay(html: string, blockRemoteContent: boolean): string {
  if (typeof DOMParser === 'undefined') return '';
  const doc = new DOMParser().parseFromString(html, 'text/html');
  doc.querySelectorAll('script,style,iframe,object,embed,form,meta,link,base').forEach((node) => node.remove());
  const allowedTags = new Set([
    'a', 'abbr', 'b', 'blockquote', 'br', 'caption', 'center', 'code', 'col', 'colgroup',
    'del', 'div', 'em', 'font', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'i', 'img',
    'ins', 'li', 'ol', 'p', 'pre', 's', 'small', 'span', 'strong', 'sub', 'sup', 'table',
    'tbody', 'td', 'tfoot', 'th', 'thead', 'tr', 'u', 'ul',
  ]);
  const allowedAttributes = new Set([
    'align', 'alt', 'border', 'cellpadding', 'cellspacing', 'colspan', 'height', 'href',
    'rowspan', 'src', 'style', 'title', 'valign', 'width',
  ]);
  doc.body.querySelectorAll('*').forEach((node) => {
    if (!allowedTags.has(node.tagName.toLowerCase())) {
      node.replaceWith(...node.childNodes);
      return;
    }
    for (const attribute of [...node.attributes]) {
      const name = attribute.name.toLowerCase();
      const value = attribute.value.trim();
      if (!allowedAttributes.has(name)) {
        node.removeAttribute(attribute.name);
        continue;
      }
      if (name === 'style') {
        // Parse declarations instead of stripping substrings: CSS escapes and
        // protocol-relative URLs must not bypass remote-content blocking.
        const style = document.createElement('span').style;
        style.cssText = value;
        const allowedStyles = /^(?:color|background-color|font-(?:family|size|weight|style)|text-(?:align|decoration|indent)|line-height|letter-spacing|white-space|word-break|overflow-wrap|border(?:-(?:top|right|bottom|left))?(?:-(?:width|style|color))?|border-collapse|border-spacing|padding(?:-(?:top|right|bottom|left))?|margin(?:-(?:top|right|bottom|left))?|(?:max-|min-)?(?:width|height)|vertical-align)$/;
        const cleaned: string[] = [];
        for (const property of Array.from(style)) {
          const declaration = style.getPropertyValue(property);
          if (allowedStyles.test(property) && !/[\\]|url\s*\(|expression\s*\(|var\s*\(/i.test(declaration)) {
            cleaned.push(`${property}:${declaration}`);
          }
        }
        if (cleaned.length) node.setAttribute('style', cleaned.join(';'));
        else node.removeAttribute('style');
      }
      if (name === 'href' && !/^(?:https?:\/\/|mailto:)/i.test(value)) {
        node.removeAttribute(attribute.name);
      }
    }
    if (node instanceof HTMLImageElement) {
      const source = node.getAttribute('src') ?? '';
      const embeddedImage = /^data:image\/(?:png|jpeg|gif|webp);base64,[a-z0-9+/=\s]+$/i.test(source);
      if (!embeddedImage && (blockRemoteContent || !/^https?:\/\//i.test(source))) {
        node.removeAttribute('src');
        node.alt = node.alt || '远程图片已阻止';
      }
    }

    if (node instanceof HTMLAnchorElement) {
      node.target = '_blank';
      node.rel = 'noopener noreferrer';
    }
  });
  return doc.body.innerHTML;
}
