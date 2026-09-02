import { useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { Mailbox, Message } from '../types';
import { MessageList } from './MessageList';
import { Reader } from './Reader';
import { useUiStore } from '../stores/ui';
import { deleteMessages, moveMessages, mutateMessage } from '../lib/tauri';

type MessageMutation = { isRead?: boolean; isStarred?: boolean };

export function MailHome({ messages, mailboxes, isLoading, onSync }: { messages: Message[]; mailboxes: Mailbox[]; isLoading: boolean; onSync?: () => void }) {
  const { selectedMessageId, selectMessage, setSyncMessage } = useUiStore();
  const queryClient = useQueryClient();
  const [optimisticFlags, setOptimisticFlags] = useState<Record<string, MessageMutation>>({});
  const [removedMessageIds, setRemovedMessageIds] = useState<Set<string>>(() => new Set());
  const localMessages = useMemo(() => messages.filter((message) => !removedMessageIds.has(message.id)).map((message) => ({ ...message, ...optimisticFlags[message.id] })), [messages, optimisticFlags, removedMessageIds]);

  const applyMutation = (messageId: string, mutation: MessageMutation) => {
    setOptimisticFlags((current) => ({ ...current, [messageId]: { ...current[messageId], ...mutation } }));
    void mutateMessage(messageId, mutation).catch(() => {
      // The local optimistic state is still useful while offline; the pending operation is retried by the desktop queue.
    });
  };

  const archiveMessage = (messageId: string) => {
    const target = mailboxes.find((mailbox) => mailbox.specialRole === 'archive') ?? mailboxes.find((mailbox) => mailbox.id === 'archive');
    if (!target) {
      setSyncMessage('当前账户未提供归档文件夹');
      return;
    }
    void moveMessages([messageId], target.id).then(() => {
      setRemovedMessageIds((current) => new Set(current).add(messageId));
      selectMessage(null);
      void queryClient.invalidateQueries({ queryKey: ['messages'] });
      void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
    }).catch(() => undefined);
  };

  const deleteMessage = (messageId: string) => {
    void deleteMessages([messageId]).then(() => {
      setRemovedMessageIds((current) => new Set(current).add(messageId));
      selectMessage(null);
      void queryClient.invalidateQueries({ queryKey: ['messages'] });
      void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
    }).catch(() => undefined);
  };

  const selectedMessage = useMemo(() => localMessages.find((message) => message.id === selectedMessageId) ?? localMessages[0], [localMessages, selectedMessageId]);
  return (
    <div className={`mail-layout ${selectedMessageId ? 'mobile-reader-open' : ''}`}>
      <section className="list-pane" aria-label="邮件列表">
        <MessageList messages={localMessages} selectedMessageId={selectedMessageId ?? undefined} onSelect={selectMessage} onToggle={applyMutation} onRefresh={onSync} isLoading={isLoading} />
      </section>
      <section className="reader-pane" aria-label="邮件阅读器">
        {selectedMessage ? <Reader message={selectedMessage} onBack={() => selectMessage(null)} onMutate={applyMutation} onArchive={archiveMessage} onDelete={deleteMessage} /> : <div className="empty-reader"><div className="empty-icon"><span>✦</span></div><h2>选一封邮件开始阅读</h2><p>你的邮件会保存在本地，断网时也能打开。</p></div>}
      </section>
    </div>
  );
}
