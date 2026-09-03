import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { Account, Mailbox, Message } from '../types';
import { MessageList } from './MessageList';
import { Reader } from './Reader';
import { useUiStore } from '../stores/ui';
import { appErrorMessage, deleteMessages, fetchMessageBody, moveMessages, mutateMessage } from '../lib/tauri';
import { Icon } from '../lib/icons';

type MessageMutation = { isRead?: boolean; isStarred?: boolean };

export function MailHome({
  hasAccounts,
  accounts,
  messages,
  mailboxes,
  isLoading,
  onSync,
  onOpenSettings,
}: {
  hasAccounts: boolean;
  accounts: Account[];
  messages: Message[];
  mailboxes: Mailbox[];
  isLoading: boolean;
  onSync?: () => void;
  onOpenSettings: () => void;
}) {
  const { selectedMessageId, selectMessage, setSyncMessage } = useUiStore();
  const queryClient = useQueryClient();
  const singlePaneQuery = '(max-width: 839px), (max-height: 479px) and (max-width: 1199px)';
  const [isSinglePane, setIsSinglePane] = useState(() => typeof window !== 'undefined' && window.matchMedia(singlePaneQuery).matches);
  const [optimisticFlags, setOptimisticFlags] = useState<Record<string, MessageMutation>>({});
  const [removedMessageIds, setRemovedMessageIds] = useState<Set<string>>(() => new Set());
  const [hydratedMessages, setHydratedMessages] = useState<Record<string, Message>>({});
  const [bodyLoadingKey, setBodyLoadingKey] = useState<string | null>(null);
  const [bodyErrors, setBodyErrors] = useState<Record<string, string>>({});
  const bodyRequests = useRef(new Set<string>());
  const fetchedBodies = useRef(new Set<string>());
  const messageInstanceKey = useCallback(
    (message: Pick<Message, 'id' | 'mailboxId'>) => `${message.id}\u0000${message.mailboxId}`,
    [],
  );
  const localMessages = useMemo(
    () => messages
      .filter((message) => !removedMessageIds.has(message.id))
      .map((message) => ({ ...message, ...hydratedMessages[messageInstanceKey(message)], ...optimisticFlags[message.id] })),
    [hydratedMessages, messageInstanceKey, messages, optimisticFlags, removedMessageIds],
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
    if (!isLoading && selectedMessageId && !localMessages.some((message) => message.id === selectedMessageId)) {
      selectMessage(null);
    }
  }, [isLoading, localMessages, selectMessage, selectedMessageId]);

  const applyMutation = (messageId: string, mutation: MessageMutation) => {
    const source = localMessages.find((message) => message.id === messageId);
    if (!source) return;
    setOptimisticFlags((current) => ({ ...current, [messageId]: { ...current[messageId], ...mutation } }));
    void mutateMessage({ messageId, mailboxId: source.mailboxId }, mutation).catch(() => {
      setOptimisticFlags((current) => {
        const next = { ...current };
        delete next[messageId];
        return next;
      });
      setSyncMessage('邮件状态更新失败');
    });
  };

  const getNextMessageId = (currentId: string) => {
    const idx = localMessages.findIndex((m) => m.id === currentId);
    if (idx === -1) return null;
    return localMessages[idx + 1]?.id ?? localMessages[idx - 1]?.id ?? null;
  };

  const archiveMessage = (messageId: string) => {
    const source = localMessages.find((message) => message.id === messageId);
    if (!source) return;
    const accountMailboxes = mailboxes.filter((mailbox) => mailbox.accountId === source.accountId);
    const target = accountMailboxes.find((mailbox) => mailbox.specialRole === 'archive');
    if (!target) {
      setSyncMessage('当前账户未提供归档文件夹');
      return;
    }
    const nextId = getNextMessageId(messageId);
    void moveMessages([{ messageId, mailboxId: source.mailboxId }], target.id).then(() => {
      setRemovedMessageIds((current) => new Set(current).add(messageId));
      selectMessage(nextId);
      void queryClient.invalidateQueries({ queryKey: ['messages'] });
      void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
    }).catch(() => undefined);
  };

  const deleteMessage = (messageId: string) => {
    const source = localMessages.find((message) => message.id === messageId);
    if (!source) return;
    const nextId = getNextMessageId(messageId);
    void deleteMessages([{ messageId, mailboxId: source.mailboxId }]).then(() => {
      setRemovedMessageIds((current) => new Set(current).add(messageId));
      selectMessage(nextId);
      void queryClient.invalidateQueries({ queryKey: ['messages'] });
      void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
    }).catch(() => undefined);
  };

  const selectedMessage = useMemo(() => {
    if (selectedMessageId) {
      return localMessages.find((message) => message.id === selectedMessageId) ?? null;
    }
    return isSinglePane ? null : localMessages[0] ?? null;
  }, [isSinglePane, localMessages, selectedMessageId]);

  const hydrateBody = useCallback((message: Message) => {
    const key = messageInstanceKey(message);
    if (message.bodyText != null || message.bodyHtmlText != null || fetchedBodies.current.has(key) || bodyRequests.current.has(key)) return;
    bodyRequests.current.add(key);
    setBodyLoadingKey(key);
    setBodyErrors((current) => ({ ...current, [key]: '' }));
    void fetchMessageBody({ messageId: message.id, mailboxId: message.mailboxId })
      .then((hydrated) => {
        fetchedBodies.current.add(key);
        setHydratedMessages((current) => ({ ...current, [key]: hydrated }));
      })
      .catch((error) => {
        setBodyErrors((current) => ({ ...current, [key]: appErrorMessage(error) }));
      })
      .finally(() => {
        bodyRequests.current.delete(key);
        setBodyLoadingKey((current) => current === key ? null : current);
      });
  }, [messageInstanceKey]);

  useEffect(() => {
    if (selectedMessage) hydrateBody(selectedMessage);
  }, [hydrateBody, selectedMessage]);

  if (!hasAccounts) {
    return (
      <section className="account-empty-state" aria-labelledby="account-empty-title">
        <div className="empty-icon"><Icon name="inbox" size={28} /></div>
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
          onToggle={applyMutation}
          onRefresh={onSync}
          isLoading={isLoading}
        />
      </section>
      <section className="reader-pane" aria-label="邮件阅读器">
        {selectedMessage ? (
          <Reader
            message={selectedMessage}
            accountEmail={accounts.find((account) => account.id === selectedMessage.accountId)?.email}
            bodyLoading={bodyLoadingKey === messageInstanceKey(selectedMessage)}
            bodyError={bodyErrors[messageInstanceKey(selectedMessage)]}
            onRetryBody={() => hydrateBody(selectedMessage)}
            onBack={() => selectMessage(null)}
            onMutate={applyMutation}
            onArchive={archiveMessage}
            onDelete={deleteMessage}
          />
        ) : (
          <div className="empty-reader">
            <div className="empty-icon"><Icon name="inbox" size={28} /></div>
            <h2>选一封邮件开始阅读</h2>
            <p>你的邮件会保存在本地，断网时也能随时打开查看。</p>
          </div>
        )}
      </section>
    </div>
  );
}
