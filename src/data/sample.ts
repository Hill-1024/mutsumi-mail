import type { Account, Mailbox, Message } from '../types';

export const sampleAccount: Account = {
  id: 'demo-account',
  providerId: 'qq',
  email: 'kobayashi@foxmail.com',
  displayName: '小林',
  enabled: true,
  syncPolicy: 'automatic',
  incomingConfigured: true,
  outgoingConfigured: true,
  syncStatus: 'idle',
  lastSyncedAt: '2026-09-02T08:40:00+08:00',
};

export const sampleMailboxes: Mailbox[] = [
  { id: 'inbox', accountId: sampleAccount.id, remoteId: 'INBOX', name: 'INBOX', displayName: '收件箱', specialRole: 'inbox', unreadCount: 12, totalCount: 128, syncEnabled: true },
  { id: 'starred', accountId: sampleAccount.id, remoteId: 'STARRED', name: 'STARRED', displayName: '星标邮件', unreadCount: 0, totalCount: 24, syncEnabled: true },
  { id: 'drafts', accountId: sampleAccount.id, remoteId: 'Drafts', name: 'Drafts', displayName: '草稿', specialRole: 'drafts', unreadCount: 0, totalCount: 3, syncEnabled: true },
  { id: 'sent', accountId: sampleAccount.id, remoteId: 'Sent', name: 'Sent', displayName: '已发送', specialRole: 'sent', unreadCount: 0, totalCount: 245, syncEnabled: true },
  { id: 'archive', accountId: sampleAccount.id, remoteId: 'Archive', name: 'Archive', displayName: '归档', specialRole: 'archive', unreadCount: 0, totalCount: 1042, syncEnabled: true },
  { id: 'trash', accountId: sampleAccount.id, remoteId: 'Trash', name: 'Trash', displayName: '回收站', specialRole: 'trash', unreadCount: 0, totalCount: 18, syncEnabled: true },
];

const message = (data: Omit<Message, 'accountId' | 'mailboxId'>): Message => ({
  ...data,
  accountId: sampleAccount.id,
  mailboxId: 'inbox',
});

export const sampleMessages: Message[] = [
  message({ id: 'msg-1', threadId: 'thread-1', messageId: '<q4-review@example.com>', subject: 'Q4 design review', normalizedSubject: 'q4 design review', from: { name: 'Takeshi Tanaka', email: 'takeshi.tanaka@example.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-09-02T08:31:00+08:00', preview: 'I have attached the latest notes from our review. The new navigation rail feels much calmer.', bodyText: 'Hi 小林,\n\nI have attached the latest notes from our review. The new navigation rail feels much calmer.\n\nCould we make the offline state a little more visible without making it feel alarming?\n\nBest,\nTakeshi', bodyHtmlText: 'Hi 小林,\n\nI have attached the latest notes from our review. The new navigation rail feels much calmer.\n\nCould we make the offline state a little more visible without making it feel alarming?\n\nBest,\nTakeshi', isRead: false, isStarred: true, hasAttachment: true, attachmentCount: 1, labels: ['design', 'review'], sizeBytes: 182_340 }),
  message({ id: 'msg-2', threadId: 'thread-2', subject: 'Re: Mutsumi Mail sync notes', normalizedSubject: 'mutsumi mail sync notes', from: { name: 'Ayaka Mori', email: 'ayaka.mori@example.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-09-02T07:56:00+08:00', preview: 'The incremental cursor survives a restart now. I also added a retry budget for IDLE.', bodyText: 'The incremental cursor survives a restart now. I also added a retry budget for IDLE.', isRead: false, isStarred: false, hasAttachment: false, labels: ['engineering'], sizeBytes: 48_220 }),
  message({ id: 'msg-3', threadId: 'thread-3', subject: 'Your September statement', normalizedSubject: 'your september statement', from: { name: 'Foxmail Billing', email: 'billing@example.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-09-01T19:20:00+08:00', preview: 'Your monthly statement is ready to view securely in your account.', bodyText: 'Your monthly statement is ready to view securely in your account.', isRead: true, isStarred: false, hasAttachment: true, attachmentCount: 1, labels: ['finance'], sizeBytes: 74_100 }),
  message({ id: 'msg-4', threadId: 'thread-4', subject: 'Team offsite · Kyoto itinerary', normalizedSubject: 'team offsite kyoto itinerary', from: { name: 'Naoko Suzuki', email: 'naoko.suzuki@example.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-09-01T16:05:00+08:00', preview: 'A quiet morning, a short walk, and a room with enough whiteboard space.', bodyText: 'A quiet morning, a short walk, and a room with enough whiteboard space.', isRead: true, isStarred: true, hasAttachment: false, labels: ['team'], sizeBytes: 62_000 }),
  message({ id: 'msg-5', threadId: 'thread-5', subject: 'Receipt for your workspace', normalizedSubject: 'receipt for your workspace', from: { name: 'Cloudflare', email: 'billing@cloudflare.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-08-31T11:42:00+08:00', preview: 'Thanks for using Email Sending. Your receipt is attached.', bodyText: 'Thanks for using Email Sending. Your receipt is attached.', isRead: true, isStarred: false, hasAttachment: true, attachmentCount: 1, labels: ['finance'], sizeBytes: 83_520 }),
  message({ id: 'msg-6', threadId: 'thread-6', subject: 'Re: Reader safety checklist', normalizedSubject: 'reader safety checklist', from: { name: 'Hana Ito', email: 'hana.ito@example.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-08-30T18:25:00+08:00', preview: 'Remote images remain blocked by default, and the allowlist is scoped to the sender.', bodyText: 'Remote images remain blocked by default, and the allowlist is scoped to the sender.', isRead: true, isStarred: false, hasAttachment: false, labels: ['security'], sizeBytes: 51_900 }),
  message({ id: 'msg-7', threadId: 'thread-7', subject: 'A small thank you', normalizedSubject: 'a small thank you', from: { name: 'Mika', email: 'mika@example.com' }, to: [{ name: '小林', email: sampleAccount.email }], date: '2026-08-30T09:12:00+08:00', preview: 'The little fox icon made my day. Thank you for caring about the details.', bodyText: 'The little fox icon made my day. Thank you for caring about the details.', isRead: true, isStarred: false, hasAttachment: false, labels: ['personal'], sizeBytes: 32_510 }),
];
