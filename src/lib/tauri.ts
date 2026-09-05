import { invoke } from '@tauri-apps/api/core';
import type {
  Account,
  AppErrorDto,
  DraftAttachment,
  DraftInput,
  Mailbox,
  Message,
  OutboxItem,
  ProviderId,
  ProviderPreset,
  SyncStatus,
  ThemePaletteId,
} from '../types';

export const isTauriRuntime = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
export interface AppSettings {
  theme: 'system' | 'light' | 'dark';
  colorScheme: ThemePaletteId;
  customThemeSeed: string;
  androidDynamicColor: boolean;
  safeReading: boolean;
  syncPolicy: string;
}

const defaultSettings: AppSettings = {
  theme: 'system',
  colorScheme: 'matcha',
  customThemeSeed: '#3F6654',
  androidDynamicColor: false,
  safeReading: true,
  syncPolicy: 'automatic',
};

const browserFallback = <T>(value: T): Promise<T> => Promise.resolve(value);

async function call<T>(command: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (isTauriRuntime) return invoke<T>(command, args);
  if (fallback !== undefined) return browserFallback(fallback);
  throw new Error('此操作需要在 Mutsumi Mail 桌面应用中完成。');
}

export async function listAccounts(): Promise<Account[]> {
  return call<Account[]>('list_accounts', undefined, []);
}

export async function listMailboxes(accountId?: string): Promise<Mailbox[]> {
  return call<Mailbox[]>('list_mailboxes', accountId ? { accountId } : {}, []);
}

export interface MessageQuery {
  accountId?: string;
  mailboxId?: string;
  mailboxRole?: Mailbox['specialRole'];
  isStarred?: boolean;
  search?: string;
  limit?: number;
}

export async function listMessages(params: MessageQuery = {}): Promise<Message[]> {
  return call<Message[]>('list_messages', { input: params }, []);
}

export async function searchMessages(
  params: MessageQuery & { search: string },
): Promise<Message[]> {
  if (!isTauriRuntime) return listMessages(params);
  return call<Message[]>('search_messages', { input: params });
}

export interface MessageInstanceRef {
  messageId: string;
  mailboxId: string;
}

export async function mutateMessage(
  message: MessageInstanceRef,
  mutation: { isRead?: boolean; isStarred?: boolean },
): Promise<Message> {
  return call<Message>('mutate_message', { ...message, mutation });
}

export async function mutateMessages(
  messages: MessageInstanceRef[],
  mutation: { isRead?: boolean; isStarred?: boolean },
): Promise<{ mutated: number }> {
  return call<{ mutated: number }>('mutate_messages', { messages, mutation });
}

export async function markRead(message: MessageInstanceRef, isRead: boolean): Promise<Message> {
  return call<Message>('mark_read', { ...message, isRead });
}

export async function setStarred(
  message: MessageInstanceRef,
  isStarred: boolean,
): Promise<Message> {
  return call<Message>('set_starred', { ...message, isStarred });
}

export async function moveMessages(
  messages: MessageInstanceRef[],
  mailboxId: string,
): Promise<{ moved: number }> {
  return call('move_messages', { messages, mailboxId });
}

export async function deleteMessages(
  messages: MessageInstanceRef[],
  permanent = false,
): Promise<{ deleted: number }> {
  return call('delete_messages', { messages, permanent });
}

export async function getMessage(messageId: string): Promise<Message> {
  return call<Message>('get_message', { messageId });
}

export async function fetchMessageBody(message: MessageInstanceRef): Promise<Message> {
  return call<Message>('fetch_message_body', { ...message });
}

export async function downloadAttachment(attachmentId: string): Promise<{
  attachment: import('../types').AttachmentInfo;
  bytes: number[];
}> {
  return call('download_attachment', { attachmentId });
}

export async function saveDraft(input: DraftInput): Promise<{ id: string; savedAt: string }> {
  return call('save_draft', { input });
}

export async function sendDraft(input: DraftInput): Promise<{ outboxId: string; state: string }> {
  return call('send_draft', { input });
}

export async function sendDraftWithAttachments(
  input: DraftInput,
  attachments: DraftAttachment[],
): Promise<{ outboxId: string; state: string }> {
  return call('send_draft_with_attachments', { input, attachments });
}

export async function listOutbox(accountId?: string): Promise<OutboxItem[]> {
  return call<OutboxItem[]>('list_outbox', accountId ? { accountId } : {}, []);
}

export async function retryOutboxItem(
  outboxId: string,
): Promise<{ outboxId: string; state: string }> {
  return call('retry_outbox_item', { outboxId });
}

export async function cancelOutboxItem(
  outboxId: string,
): Promise<{ outboxId: string; state: string }> {
  return call('cancel_outbox_item', { outboxId });
}

export async function loadDraft(draftId: string): Promise<DraftInput> {
  return call('load_draft', { draftId });
}

export async function deleteDraft(draftId: string): Promise<{ draftId: string; deleted: boolean }> {
  return call('delete_draft', { draftId });
}

export async function getSettings(): Promise<AppSettings> {
  return call<AppSettings>('get_settings', undefined, defaultSettings);
}

export async function updateSettings(settings: Record<string, unknown>): Promise<AppSettings> {
  return call('update_settings', { settings }, { ...defaultSettings, ...settings } as AppSettings);
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
  incoming?: {
    protocol: 'imap' | 'pop3' | 'jmap';
    host: string;
    port: number;
    tlsMode: 'implicit' | 'starttls';
    authMethod: string;
    username: string;
  };
  outgoing?: {
    protocol: 'smtp' | 'api' | 'jmap';
    host: string;
    port: number;
    tlsMode: 'implicit' | 'starttls';
    authMethod: string;
    username: string;
  };
}): Promise<Account> {
  return call('create_account', { input });
}

export async function removeAccount(accountId: string): Promise<void> {
  await call('remove_account', { accountId });
}

export async function startSync(accountId?: string): Promise<SyncStatus> {
  if (!accountId) throw new Error('请先添加邮箱账户');
  return call('start_sync', { accountId });
}

export async function syncAll(): Promise<SyncStatus[]> {
  return call<SyncStatus[]>('sync_all');
}

export async function detectProvider(email: string): Promise<ProviderPreset | null> {
  return call<ProviderPreset | null>('detect_provider', { email }, providerFor(email));
}

export async function getProviderPresets(): Promise<ProviderPreset[]> {
  return call('get_provider_presets', undefined, providerPresets);
}

export async function testIncomingConnection(
  accountId: string,
): Promise<{ backend: string; capabilities: Record<string, boolean>; greeting?: string }> {
  return call('test_incoming_connection', { accountId });
}

export async function testOutgoingConnection(accountId: string): Promise<void> {
  await call('test_outgoing_connection', { accountId });
}

export function appErrorMessage(error: unknown): string {
  if (typeof error === 'object' && error && 'message' in error)
    return String((error as AppErrorDto).message);
  return error instanceof Error ? error.message : '发生未知错误，请稍后重试';
}

export const providerPresets: ProviderPreset[] = [
  {
    id: 'qq',
    displayName: 'QQ 邮箱',
    emailDomainPatterns: ['qq.com', 'foxmail.com'],
    incoming: {
      protocol: 'imap',
      host: 'imap.qq.com',
      port: 993,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    outgoing: {
      protocol: 'smtp',
      host: 'smtp.qq.com',
      port: 465,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    helpText: '使用客户端授权码，不是 QQ 登录密码。请先在邮箱设置中开启 IMAP/SMTP。',
    capabilities: {
      folders: true,
      flags: true,
      partialFetch: true,
      threading: true,
      smtpUtf8: true,
    },
    quirks: ['客户端授权码', '完整邮箱地址作为用户名'],
  },
  {
    id: 'netease-163',
    displayName: '网易 163 邮箱',
    emailDomainPatterns: ['163.com'],
    incoming: {
      protocol: 'imap',
      host: 'imap.163.com',
      port: 993,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    outgoing: {
      protocol: 'smtp',
      host: 'smtp.163.com',
      port: 465,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    helpText: '使用客户端授权码，不是网页登录密码。请先在邮箱设置中开启 IMAP/SMTP。',
    capabilities: {
      folders: true,
      flags: true,
      partialFetch: true,
      threading: true,
      smtpUtf8: true,
    },
    quirks: ['客户端授权码', '可能需要重新生成授权码'],
  },
  {
    id: 'generic',
    displayName: '通用 IMAP + SMTP',
    emailDomainPatterns: [],
    incoming: {
      protocol: 'imap',
      host: '',
      port: 993,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    outgoing: {
      protocol: 'smtp',
      host: '',
      port: 465,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    helpText: '为标准邮件服务器手动填写 IMAP 与 SMTP 端点。当前使用同一密码或授权码验证收发连接。',
    capabilities: {
      folders: true,
      flags: true,
      partialFetch: true,
      threading: true,
      smtpUtf8: true,
    },
    quirks: [],
  },
  {
    id: 'generic-smtp',
    displayName: '通用 SMTP（仅发件）',
    emailDomainPatterns: [],
    outgoing: {
      protocol: 'smtp',
      host: '',
      port: 465,
      tlsMode: 'implicit',
      authMethods: ['password'],
    },
    helpText: '手动填写 SMTP 发件端点。此账户只用于发件，不会收取邮件或同步“已发送”副本。',
    capabilities: { smtpUtf8: true },
    quirks: ['仅发件'],
  },
  {
    id: 'cloudflare-smtp',
    displayName: 'Cloudflare Email Sending',
    emailDomainPatterns: ['cloudflare.email'],
    outgoing: {
      protocol: 'smtp',
      host: 'smtp.mx.cloudflare.net',
      port: 465,
      tlsMode: 'implicit',
      authMethods: ['api-token'],
      username: 'api_token',
    },
    helpText:
      '这是 outbound-only 发件 preset，不是完整邮箱服务。用户名固定为 api_token，密码为具有 Email Sending: Edit 权限的 API Token。',
    capabilities: { smtpUtf8: true },
    quirks: ['仅发件', 'API Token 进入安全存储'],
  },
];

function providerFor(email: string): ProviderPreset | null {
  const domain = email.trim().toLowerCase().split('@')[1] ?? '';
  return (
    providerPresets.find((preset) =>
      preset.emailDomainPatterns.some(
        (pattern) => domain === pattern || domain.endsWith(`.${pattern}`),
      ),
    ) ?? null
  );
}
