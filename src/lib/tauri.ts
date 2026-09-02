import { invoke } from '@tauri-apps/api/core';
import type { Account, AppErrorDto, DraftInput, Mailbox, Message, OutboxItem, ProviderId, ProviderPreset, SyncStatus } from '../types';
import { sampleAccount, sampleMailboxes, sampleMessages } from '../data/sample';

export const isTauriRuntime = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
const browserOutbox: OutboxItem[] = [];
const browserAccounts: Account[] = [sampleAccount];
const browserSettings: { theme: 'system' | 'light' | 'dark'; safeReading: boolean; syncPolicy: string } = { theme: 'system', safeReading: true, syncPolicy: 'automatic' };

const browserFallback = <T,>(value: T): Promise<T> => Promise.resolve(value);

async function call<T>(command: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (isTauriRuntime) return invoke<T>(command, args);
  if (fallback !== undefined) return browserFallback(fallback);
  throw new Error(`Command ${command} requires the Tauri desktop runtime.`);
}

export async function listAccounts(): Promise<Account[]> {
  return call<Account[]>('list_accounts', undefined, browserAccounts.slice());
}

export async function listMailboxes(accountId = sampleAccount.id): Promise<Mailbox[]> {
  return call<Mailbox[]>('list_mailboxes', { accountId }, sampleMailboxes);
}

export async function listMessages(params: { mailboxId?: string; search?: string; limit?: number } = {}): Promise<Message[]> {
  const fallback = sampleMessages.filter((message) => {
    const query = params.search?.trim().toLowerCase();
    return (!params.mailboxId || params.mailboxId === 'inbox' || message.mailboxId === params.mailboxId) && (!query || `${message.subject} ${message.preview} ${message.from.email}`.toLowerCase().includes(query));
  });
  return call<Message[]>('list_messages', { input: params }, fallback.slice(0, params.limit ?? 100));
}

export async function searchMessages(params: { mailboxId?: string; search: string; limit?: number }): Promise<Message[]> {
  if (!isTauriRuntime) return listMessages(params);
  return call<Message[]>('search_messages', { input: params });
}

export async function mutateMessage(messageId: string, mutation: { isRead?: boolean; isStarred?: boolean }): Promise<Message> {
  if (!isTauriRuntime) {
    const current = sampleMessages.find((message) => message.id === messageId);
    return Promise.resolve({ ...(current ?? sampleMessages[0]), ...mutation });
  }
  return invoke<Message>('mutate_message', { messageId, mutation });
}

export async function markRead(messageId: string, isRead: boolean): Promise<Message> {
  if (!isTauriRuntime) return mutateMessage(messageId, { isRead });
  return invoke<Message>('mark_read', { messageId, isRead });
}

export async function setStarred(messageId: string, isStarred: boolean): Promise<Message> {
  if (!isTauriRuntime) return mutateMessage(messageId, { isStarred });
  return invoke<Message>('set_starred', { messageId, isStarred });
}

export async function moveMessages(messageIds: string[], mailboxId: string): Promise<{ moved: number }> {
  if (!isTauriRuntime) return { moved: messageIds.length };
  return invoke('move_messages', { messageIds, mailboxId });
}

export async function deleteMessages(messageIds: string[], permanent = false): Promise<{ deleted: number }> {
  if (!isTauriRuntime) return { deleted: messageIds.length };
  return invoke('delete_messages', { messageIds, permanent });
}

export async function getMessage(messageId: string): Promise<Message> {
  return call<Message>('get_message', { messageId }, sampleMessages.find((message) => message.id === messageId) ?? sampleMessages[0]);
}

export async function saveDraft(input: DraftInput): Promise<{ id: string; savedAt: string }> {
  return call('save_draft', { input }, { id: input.id ?? `draft-${Date.now()}`, savedAt: new Date().toISOString() });
}

export async function sendDraft(input: DraftInput): Promise<{ outboxId: string; state: string }> {
  const outboxId = `outbox-${Date.now()}`;
  if (!isTauriRuntime) browserOutbox.unshift({ id: outboxId, accountId: input.accountId, subject: input.subject, recipients: input.to.split(',').map((value) => value.trim()).filter(Boolean), state: 'queued', updatedAt: new Date().toISOString() });
  return call('send_draft', { input }, { outboxId, state: 'queued' });
}

export async function listOutbox(accountId?: string): Promise<OutboxItem[]> {
  return call<OutboxItem[]>('list_outbox', { accountId }, browserOutbox.filter((item) => !accountId || item.accountId === accountId));
}

export async function retryOutboxItem(outboxId: string): Promise<{ outboxId: string; state: string }> {
  return call('retry_outbox_item', { outboxId }, { outboxId, state: 'queued' });
}

export async function cancelOutboxItem(outboxId: string): Promise<{ outboxId: string; state: string }> {
  return call('cancel_outbox_item', { outboxId }, { outboxId, state: 'cancelled' });
}

export async function loadDraft(draftId: string): Promise<DraftInput> {
  return call('load_draft', { draftId }, { id: draftId, accountId: sampleAccount.id, to: '', subject: '', bodyText: '' });
}

export async function deleteDraft(draftId: string): Promise<{ draftId: string; deleted: boolean }> {
  return call('delete_draft', { draftId }, { draftId, deleted: true });
}

export async function getSettings(): Promise<{ theme: 'system' | 'light' | 'dark'; safeReading: boolean; syncPolicy: string }> {
  return call<{ theme: 'system' | 'light' | 'dark'; safeReading: boolean; syncPolicy: string }>('get_settings', undefined, browserSettings);
}

export async function updateSettings(settings: Record<string, unknown>): Promise<{ theme: string; safeReading: boolean; syncPolicy: string }> {
  if (!isTauriRuntime && typeof settings.theme === 'string' && ['system', 'light', 'dark'].includes(settings.theme)) browserSettings.theme = settings.theme as 'system' | 'light' | 'dark';
  return call('update_settings', { settings }, browserSettings);
}

export async function clearCache(): Promise<{ deletedMessages: number }> {
  return call('clear_cache', undefined, { deletedMessages: 0 });
}

export async function exportDiagnostics(): Promise<Record<string, unknown>> {
  return call('export_diagnostics', undefined, { app: 'Mutsumi Mail', schema: 1 });
}

export async function createAccount(input: {
  email: string;
  displayName: string;
  providerId: ProviderId;
  secret: string;
  incomingSecret?: string;
  outgoingSecret?: string;
  incoming?: { protocol: 'imap' | 'pop3' | 'jmap'; host: string; port: number; tlsMode: 'implicit' | 'starttls'; authMethod: string; username: string };
  outgoing?: { protocol: 'smtp' | 'api' | 'jmap'; host: string; port: number; tlsMode: 'implicit' | 'starttls'; authMethod: string; username: string };
}): Promise<Account> {
  const fallback: Account = { id: `account-${Date.now()}`, providerId: input.providerId, email: input.email, displayName: input.displayName, enabled: true, syncPolicy: 'automatic', incomingConfigured: input.providerId !== 'cloudflare-smtp', outgoingConfigured: true, syncStatus: 'idle' };
  if (!isTauriRuntime) browserAccounts.unshift(fallback);
  return call('create_account', { input }, fallback);
}

export async function startSync(accountId?: string): Promise<SyncStatus> {
  return call('start_sync', { accountId }, { accountId: accountId ?? sampleAccount.id, state: 'syncing', phase: 'metadata', processed: 0, total: 128 });
}

export async function detectProvider(email: string): Promise<ProviderPreset | null> {
  return call<ProviderPreset | null>('detect_provider', { email }, providerFor(email));
}

export async function getProviderPresets(): Promise<ProviderPreset[]> {
  return call('get_provider_presets', undefined, providerPresets);
}

export async function testIncomingConnection(accountId: string): Promise<{ backend: string; capabilities: Record<string, boolean>; greeting?: string }> {
  if (!isTauriRuntime) return { backend: 'browser-preview', capabilities: {}, greeting: '桌面运行时才会连接真实服务器' };
  return invoke('test_incoming_connection', { accountId });
}

export async function testOutgoingConnection(accountId: string): Promise<void> {
  if (!isTauriRuntime) return;
  await invoke('test_outgoing_connection', { accountId });
}

export function appErrorMessage(error: unknown): string {
  if (typeof error === 'object' && error && 'message' in error) return String((error as AppErrorDto).message);
  return error instanceof Error ? error.message : '发生未知错误，请稍后重试';
}

export const providerPresets: ProviderPreset[] = [
  {
    id: 'qq', displayName: 'QQ 邮箱', emailDomainPatterns: ['qq.com', 'foxmail.com'],
    incoming: { protocol: 'imap', host: 'imap.qq.com', port: 993, tlsMode: 'implicit', authMethods: ['password'] },
    outgoing: { protocol: 'smtp', host: 'smtp.qq.com', port: 465, tlsMode: 'implicit', authMethods: ['password'] },
    helpText: '使用客户端授权码，不是 QQ 登录密码。请先在邮箱设置中开启 IMAP/SMTP。',
    capabilities: { folders: true, flags: true, move: true, appendSent: true, partialFetch: true, threading: true, smtpUtf8: true },
    quirks: ['客户端授权码', '完整邮箱地址作为用户名'],
  },
  {
    id: 'netease-163', displayName: '网易 163 邮箱', emailDomainPatterns: ['163.com'],
    incoming: { protocol: 'imap', host: 'imap.163.com', port: 993, tlsMode: 'implicit', authMethods: ['password'] },
    outgoing: { protocol: 'smtp', host: 'smtp.163.com', port: 465, tlsMode: 'implicit', authMethods: ['password'] },
    helpText: '使用客户端授权码，不是网页登录密码。请先在邮箱设置中开启 IMAP/SMTP。',
    capabilities: { folders: true, flags: true, move: true, appendSent: true, partialFetch: true, threading: true, smtpUtf8: true },
    quirks: ['客户端授权码', '可能需要重新生成授权码'],
  },
  {
    id: 'generic', displayName: '通用 IMAP + SMTP', emailDomainPatterns: [],
    incoming: { protocol: 'imap', host: '', port: 993, tlsMode: 'implicit', authMethods: ['password', 'oauth2', 'xoauth2'] },
    outgoing: { protocol: 'smtp', host: '', port: 465, tlsMode: 'implicit', authMethods: ['password', 'oauth2', 'xoauth2'] },
    helpText: '为标准邮件服务器手动填写收件和发件端点；两者凭据可以不同。',
    capabilities: { folders: true, flags: true, move: true, appendSent: true, partialFetch: true, threading: true, smtpUtf8: true },
    quirks: [],
  },
  {
    id: 'cloudflare-smtp', displayName: 'Cloudflare Email Sending', emailDomainPatterns: ['cloudflare.email'],
    outgoing: { protocol: 'smtp', host: 'smtp.mx.cloudflare.net', port: 465, tlsMode: 'implicit', authMethods: ['api-token'], username: 'api_token' },
    helpText: '这是 outbound-only 发件 preset，不是完整邮箱服务。用户名固定为 api_token，密码为具有 Email Sending: Edit 权限的 API Token。',
    capabilities: { smtpUtf8: true },
    quirks: ['仅发件', 'API Token 进入安全存储'],
  },
];

function providerFor(email: string): ProviderPreset | null {
  const domain = email.trim().toLowerCase().split('@')[1] ?? '';
  return providerPresets.find((preset) => preset.emailDomainPatterns.some((pattern) => domain === pattern || domain.endsWith(`.${pattern}`))) ?? null;
}
