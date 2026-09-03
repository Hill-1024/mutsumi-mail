export type ThemeMode = 'dark' | 'light' | 'system';

export type ProviderId = 'qq' | 'netease-163' | 'generic' | 'generic-smtp' | 'cloudflare-smtp';

export type CapabilityName =
  | 'folders'
  | 'labels'
  | 'idlePush'
  | 'serverSearch'
  | 'move'
  | 'copy'
  | 'append'
  | 'appendSent'
  | 'drafts'
  | 'trash'
  | 'archive'
  | 'flags'
  | 'keywords'
  | 'threading'
  | 'partialFetch'
  | 'smtpUtf8'
  | 'oauth2'
  | 'multipleIdentities';

export type ProviderCapabilities = Partial<Record<CapabilityName, boolean>>;

export interface EndpointPreset {
  host: string;
  port: number;
  tlsMode: 'implicit' | 'starttls';
  authMethods: Array<'password' | 'oauth2' | 'xoauth2' | 'api-token'>;
  username?: string;
}

export interface ProviderPreset {
  id: ProviderId;
  displayName: string;
  emailDomainPatterns: string[];
  incoming?: EndpointPreset & { protocol: 'imap' | 'pop3' | 'jmap' };
  outgoing?: EndpointPreset & { protocol: 'smtp' | 'api' | 'jmap' };
  helpText: string;
  capabilities: ProviderCapabilities;
  quirks: string[];
}

export interface Account {
  id: string;
  providerId: ProviderId;
  email: string;
  displayName: string;
  enabled: boolean;
  syncPolicy: 'automatic' | 'manual' | 'paused';
  incomingConfigured: boolean;
  outgoingConfigured: boolean;
  syncStatus: 'idle' | 'syncing' | 'offline' | 'error';
  lastSyncedAt?: string;
}

export interface Mailbox {
  id: string;
  accountId: string;
  remoteId: string;
  name: string;
  displayName: string;
  specialRole?: 'inbox' | 'sent' | 'drafts' | 'trash' | 'junk' | 'archive' | 'all' | 'starred';
  unreadCount: number;
  totalCount: number;
  syncEnabled: boolean;
}

export interface Address {
  name?: string;
  email: string;
}

export interface Message {
  id: string;
  accountId: string;
  mailboxId: string;
  threadId: string;
  messageId?: string;
  subject: string;
  normalizedSubject: string;
  from: Address;
  to: Address[];
  date: string;
  preview: string;
  bodyText?: string;
  bodyHtmlText?: string;
  isRead: boolean;
  isStarred: boolean;
  hasAttachment: boolean;
  attachmentCount?: number;
  labels: string[];
  sizeBytes?: number;
}

export interface SyncStatus {
  accountId: string;
  state: 'idle' | 'syncing' | 'partial' | 'offline' | 'error';
  phase?: 'authentication' | 'folders' | 'messages' | 'metadata' | 'body' | 'outbox';
  processed?: number;
  total?: number;
  message?: string;
  retryable?: boolean;
}

export interface DraftInput {
  id?: string;
  accountId: string;
  to: string;
  cc?: string;
  bcc?: string;
  subject: string;
  bodyText: string;
  inReplyTo?: string;
  references?: string[];
}

export interface OutboxItem {
  id: string;
  accountId: string;
  subject: string;
  recipients: string[];
  state: 'queued' | 'sending' | 'sent' | 'failed' | 'outcome_unknown' | 'cancelled';
  lastErrorCode?: string;
  lastErrorMessage?: string;
  sentCopyState?: 'not_started' | 'awaiting_server_sync' | 'confirmed' | 'unavailable' | 'failed';
  sentCopyErrorMessage?: string;
  updatedAt: string;
}

export interface AppErrorDto {
  code: string;
  message: string;
  userAction?: string;
  retryable: boolean;
  accountId?: string;
  providerCode?: string;
  technicalDetails?: string;
}
