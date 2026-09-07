import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { Account, Mailbox, Message } from '../types';
import { MessageList } from './MessageList';
import { Reader } from './Reader';
import { useUiStore } from '../stores/ui';
import { applyFlagMutation, messageInstanceKey, readMailData } from '../lib/optimistic-flags';
import {
  appErrorMessage,
  deleteMessages,
  fetchMessageBody,
  moveMessages,
} from '../lib/tauri';
import { Icon } from '../lib/icons';

type MessageMutation = { isRead?: boolean; isStarred?: boolean };

export function MailHome({
  hasAccounts,
  accounts,
  messages,
  mailboxes,
  isLoading,
  isRefreshing = false,
  loadError,
  onRetry,
  onSync,
  onOpenSettings,
}: {
  hasAccounts: boolean;
  accounts: Account[];
  messages: Message[];
  mailboxes: Mailbox[];
  isLoading: boolean;
  /** True while placeholder (previous folder) data is on screen during a refetch. */
  isRefreshing?: boolean;
  loadError?: string;
  onRetry?: () => void;
  onSync?: () => void;
  onOpenSettings: () => void;
}) {
  const { selectedMessageId, selectMessage, setSyncMessage } = useUiStore();
  const queryClient = useQueryClient();
  const singlePaneQuery = '(max-width: 839px), (max-height: 479px) and (max-width: 1199px)';
  const [isSinglePane, setIsSinglePane] = useState(
    () => typeof window !== 'undefined' && window.matchMedia(singlePaneQuery).matches,
  );
  const [removedMessageKeys, setRemovedMessageKeys] = useState<Set<string>>(() => new Set());
  const [hydratedMessages, setHydratedMessages] = useState<Record<string, Message>>({});
  const [loadingBodyKeys, setLoadingBodyKeys] = useState<Set<string>>(() => new Set());
  const [bodyErrors, setBodyErrors] = useState<Record<string, string>>({});
  const bodyRequests = useRef(new Set<string>());
  const removalRequests = useRef(new Set<string>());
  const fetchedBodies = useRef(new Set<string>());
  const localMessages = useMemo(
    () =>
      messages
        .filter((message) => !removedMessageKeys.has(messageInstanceKey(message)))
        .map((message) => ({
          ...message,
          ...(hydratedMessages[messageInstanceKey(message)] ? {
            bodyText: hydratedMessages[messageInstanceKey(message)].bodyText,
            bodyHtmlText: hydratedMessages[messageInstanceKey(message)].bodyHtmlText,
            bodyNeedsRefresh: hydratedMessages[messageInstanceKey(message)].bodyNeedsRefresh,
            attachments: hydratedMessages[messageInstanceKey(message)].attachments,
            attachmentCount: hydratedMessages[messageInstanceKey(message)].attachmentCount,
            hasAttachment: hydratedMessages[messageInstanceKey(message)].hasAttachment,
          } : {}),
        })),
    [hydratedMessages, messages, removedMessageKeys],
  );

  useEffect(() => {
    const media = window.matchMedia(singlePaneQuery);
    const updateWindowClass = (event: MediaQueryListEvent) => setIsSinglePane(event.matches);
    media.addEventListener('change', updateWindowClass);
    return () => media.removeEventListener('change', updateWindowClass);
  }, []);

  useEffect(() => {
    if (!isSinglePane || !selectedMessageId) return undefined;
    const returnToList = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !useUiStore.getState().composeOpen) selectMessage(null);
    };
    window.addEventListener('keydown', returnToList);
    return () => window.removeEventListener('keydown', returnToList);
  }, [isSinglePane, selectMessage, selectedMessageId]);

  useEffect(() => {
    if (
      !isLoading &&
      !isRefreshing &&
      selectedMessageId &&
      !localMessages.some((message) => message.id === selectedMessageId)
    ) {
      selectMessage(null);
    }
  }, [isLoading, isRefreshing, localMessages, selectMessage, selectedMessageId]);

  const applyBulkMutation = useCallback(
    async (selectedMessages: Message[], mutation: MessageMutation) => {
      if (isRefreshing || selectedMessages.some((message) => removalRequests.current.has(messageInstanceKey(message)))) return;
      try {
        await applyFlagMutation(queryClient, selectedMessages, mutation);
      } catch (error) {
        setSyncMessage(appErrorMessage(error));
        throw error;
      }
    },
    [isRefreshing, queryClient, setSyncMessage],
  );

  const applyMutation = useCallback(
    async (messageId: string, mutation: MessageMutation) => {
      const source = localMessages.find((message) => message.id === messageId);
      if (source) await applyBulkMutation([source], mutation);
    },
    [applyBulkMutation, localMessages],
  );

  const getNextMessageId = (currentId: string) => {
    const idx = localMessages.findIndex((m) => m.id === currentId);
    if (idx === -1) return null;
    return localMessages[idx + 1]?.id ?? localMessages[idx - 1]?.id ?? null;
  };

  const archiveMessage = (messageId: string) => {
    const source = localMessages.find((message) => message.id === messageId);
    if (!source || isRefreshing) return;
    const accountMailboxes = mailboxes.filter((mailbox) => mailbox.accountId === source.accountId);
    const target = accountMailboxes.find((mailbox) => mailbox.specialRole === 'archive');
    if (!target) {
      setSyncMessage('当前账户未提供归档文件夹');
      return;
    }
    if (target.id === source.mailboxId) return;
    const key = messageInstanceKey(source);
    if (removalRequests.current.has(key)) return;
    removalRequests.current.add(key);
    const scope = useUiStore.getState().selectedMailboxId;
    const nextId = getNextMessageId(messageId);
    void readMailData(queryClient, () => moveMessages([{ messageId, mailboxId: source.mailboxId }], target.id))
      .then(async (result) => {
        if (result.moved !== 1) throw new Error('邮件未能归档');
        setRemovedMessageKeys((current) => new Set(current).add(messageInstanceKey(source)));
        const current = useUiStore.getState();
        if (current.selectedMailboxId === scope && current.selectedMessageId === messageId) selectMessage(nextId);
        await queryClient.invalidateQueries({ queryKey: ['messages'] });
        void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
        void queryClient.invalidateQueries({ queryKey: ['search'] });
        setRemovedMessageKeys((current) => { const next = new Set(current); next.delete(messageInstanceKey(source)); return next; });
      })
      .catch((error) => setSyncMessage(appErrorMessage(error)))
      .finally(() => removalRequests.current.delete(key));
  };

  const deleteSelectedMessages = useCallback(
    async (selectedMessages: Message[], nextSelection: string | null = null) => {
      const uniqueMessages = Array.from(
        new Map(selectedMessages.map((message) => [messageInstanceKey(message), message])),
      ).map(([, message]) => message);
      if (uniqueMessages.length === 0 || isRefreshing) return;
      const keys = uniqueMessages.map(messageInstanceKey);
      if (keys.some((key) => removalRequests.current.has(key))) return;
      keys.forEach((key) => removalRequests.current.add(key));
      const scope = useUiStore.getState().selectedMailboxId;
      try {
        const result = await readMailData(queryClient, () => deleteMessages(
          uniqueMessages.map((message) => ({
            messageId: message.id,
            mailboxId: message.mailboxId,
          })),
        ));
        if (result.deleted !== uniqueMessages.length) {
          throw new Error('部分邮件未能移至回收站');
        }
        setRemovedMessageKeys((current) => {
          const next = new Set(current);
          for (const message of uniqueMessages) next.add(messageInstanceKey(message));
          return next;
        });
        const current = useUiStore.getState();
        if (current.selectedMailboxId === scope && uniqueMessages.some((message) => message.id === current.selectedMessageId)) {
          selectMessage(nextSelection);
        }
        await queryClient.invalidateQueries({ queryKey: ['messages'] });
        void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
        void queryClient.invalidateQueries({ queryKey: ['search'] });
        setRemovedMessageKeys((current) => { const next = new Set(current); for (const message of uniqueMessages) next.delete(messageInstanceKey(message)); return next; });
      } catch (error) {
        setSyncMessage(appErrorMessage(error));
        throw error;
      } finally {
        keys.forEach((key) => removalRequests.current.delete(key));
      }
    },
    [isRefreshing, queryClient, selectMessage, setSyncMessage],
  );

  const deleteMessage = (messageId: string) => {
    const source = localMessages.find((message) => message.id === messageId);
    if (!source) return;
    const nextId = getNextMessageId(messageId);
    void deleteSelectedMessages([source], nextId).catch(() => undefined);
  };

  const selectedMessage = useMemo(() => {
    if (selectedMessageId) {
      return localMessages.find((message) => message.id === selectedMessageId) ?? null;
    }
    return isSinglePane ? null : (localMessages[0] ?? null);
  }, [isSinglePane, localMessages, selectedMessageId]);

  // Auto mark-read fires only when a message BECOMES the viewed one. Watching
  // isRead flips instead would let it stomp the user: toggling the selected
  // message back to unread re-triggered an auto-read that instantly reverted
  // the toggle (the "first click always fails" bug).
  const lastAutoReadSelection = useRef<string | null>(null);
  useEffect(() => {
    if (isRefreshing || isLoading) return;
    if (!selectedMessage) {
      lastAutoReadSelection.current = null;
      return;
    }
    const becameSelected = lastAutoReadSelection.current !== messageInstanceKey(selectedMessage);
    lastAutoReadSelection.current = messageInstanceKey(selectedMessage);
    if (!becameSelected || selectedMessage.isRead) return;
    void applyMutation(selectedMessage.id, { isRead: true }).catch(() => undefined);
  }, [applyMutation, isLoading, isRefreshing, selectedMessage]);

  const hydrateBody = useCallback(
    (message: Message) => {
      const key = messageInstanceKey(message);
      if (
        (!message.bodyNeedsRefresh && (message.bodyText != null || message.bodyHtmlText != null) && (!message.hasAttachment || message.attachments !== undefined)) ||
        fetchedBodies.current.has(key) ||
        bodyRequests.current.has(key)
      )
        return;
      bodyRequests.current.add(key);
      setLoadingBodyKeys((current) => new Set(current).add(key));
      setBodyErrors((current) => ({ ...current, [key]: '' }));
      void fetchMessageBody({ messageId: message.id, mailboxId: message.mailboxId })
        .then((hydrated) => {
          if (messageInstanceKey(hydrated) !== key) throw new Error('邮件正文与请求的邮件不一致');
          fetchedBodies.current.add(key);
          setHydratedMessages((current) => ({ ...current, [key]: hydrated }));
        })
        .catch((error) => {
          setBodyErrors((current) => ({ ...current, [key]: appErrorMessage(error) }));
        })
        .finally(() => {
          bodyRequests.current.delete(key);
          setLoadingBodyKeys((current) => { const next = new Set(current); next.delete(key); return next; });
        });
    },
    [],
  );

  useEffect(() => {
    if (selectedMessage && !isRefreshing) hydrateBody(selectedMessage);
  }, [hydrateBody, isRefreshing, selectedMessage]);

  if (!hasAccounts) {
    return (
      <section className="account-empty-state" aria-labelledby="account-empty-title">
        <div className="empty-icon">
          <Icon name="inbox" size={28} />
        </div>
        <h2 id="account-empty-title">尚未添加邮箱</h2>
        <p>在设置中添加邮箱。连接验证成功后，邮件才会出现在这里。</p>
        <button className="primary-action" type="button" onClick={onOpenSettings}>
          打开设置
        </button>
      </section>
    );
  }

  return (
    <div className={`mail-layout ${selectedMessageId ? 'mobile-reader-open' : ''}`}>
      <section className="list-pane" aria-label="邮件列表">
        <MessageList
          accounts={accounts}
          messages={localMessages}
          selectedMessageId={selectedMessage?.id}
          onSelect={selectMessage}
          onToggle={(id, mutation) => { void applyMutation(id, mutation).catch(() => undefined); }}
          onBulkMutate={applyBulkMutation}
          onBulkDelete={deleteSelectedMessages}
          onRefresh={onSync}
          loadError={loadError}
          onRetry={onRetry}
          isLoading={isLoading}
          isRefreshing={isRefreshing}
        />
      </section>
      <section className="reader-pane" aria-label="邮件阅读器" inert={isRefreshing || undefined}>
        {selectedMessage ? (
          <Reader
            key={messageInstanceKey(selectedMessage)}
            message={selectedMessage}
            accountEmail={
              accounts.find((account) => account.id === selectedMessage.accountId)?.email
            }
            bodyLoading={loadingBodyKeys.has(messageInstanceKey(selectedMessage))}
            bodyError={bodyErrors[messageInstanceKey(selectedMessage)]}
            onRetryBody={() => hydrateBody(selectedMessage)}
            onBack={() => selectMessage(null)}
            onMutate={(id, mutation) => { void applyMutation(id, mutation).catch(() => undefined); }}
            onArchive={archiveMessage}
            onDelete={deleteMessage}
          />
        ) : (
          <div className="empty-reader">
            <div className="empty-icon">
              <Icon name="inbox" size={28} />
            </div>
            <h2>选一封邮件开始阅读</h2>
            <p>你的邮件会保存在本地，断网时也能随时打开查看。</p>
          </div>
        )}
      </section>
    </div>
  );
}
