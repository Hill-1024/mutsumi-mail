import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useNavigate } from 'react-router-dom';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { Account, Message } from '../types';
import { Icon } from '../lib/icons';
import { useUiStore } from '../stores/ui';

interface MessageListProps {
  accounts: Account[];
  messages: Message[];
  selectedMessageId?: string;
  onSelect: (id: string) => void;
  onToggle: (messageId: string, mutation: { isRead?: boolean; isStarred?: boolean }) => void;
  onBulkMutate: (
    messages: Message[],
    mutation: { isRead?: boolean; isStarred?: boolean },
  ) => Promise<void> | void;
  onBulkDelete: (messages: Message[]) => Promise<void> | void;
  onRefresh?: () => void;
  isLoading?: boolean;
}

const messageInstanceKey = (message: Pick<Message, 'id' | 'mailboxId'>) =>
  `${message.id}\u0000${message.mailboxId}`;

const formatTime = (value: string) => {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  if (isToday) {
    return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  }
  const isThisYear = date.getFullYear() === now.getFullYear();
  if (isThisYear) {
    return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
  }
  return date.toLocaleDateString('zh-CN', { year: 'numeric', month: 'short', day: 'numeric' });
};

export function MessageList({
  accounts,
  messages,
  selectedMessageId,
  onSelect,
  onToggle,
  onBulkMutate,
  onBulkDelete,
  onRefresh,
  isLoading,
}: MessageListProps) {
  const parentRef = useRef<HTMLDivElement>(null);
  const { setSearchOpen, setNavPage } = useUiStore();
  const navigate = useNavigate();
  const [bulkMode, setBulkMode] = useState(false);
  const [selectedMessageKeys, setSelectedMessageKeys] = useState<Set<string>>(() => new Set());
  const [bulkActionPending, setBulkActionPending] = useState(false);
  const openSearch = () => {
    setSearchOpen(true);
    setNavPage('search');
    navigate('/search');
  };

  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 92,
    overscan: 8,
  });

  const unreadCount = messages.filter((message) => !message.isRead).length;
  const selectedMessages = useMemo(
    () => messages.filter((message) => selectedMessageKeys.has(messageInstanceKey(message))),
    [messages, selectedMessageKeys],
  );
  const hasSelection = selectedMessages.length > 0;

  useEffect(() => {
    const availableKeys = new Set(messages.map(messageInstanceKey));
    setSelectedMessageKeys((current) => {
      const next = new Set([...current].filter((key) => availableKeys.has(key)));
      return next.size === current.size ? current : next;
    });
  }, [messages]);

  const toggleSelection = (message: Message) => {
    const key = messageInstanceKey(message);
    setSelectedMessageKeys((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const enterBulkMode = () => setBulkMode(true);
  const selectAll = () => setSelectedMessageKeys(new Set(messages.map(messageInstanceKey)));
  const clearSelection = () => setSelectedMessageKeys(new Set());
  const exitBulkMode = () => {
    clearSelection();
    setBulkMode(false);
  };

  const runBulkMutation = (mutation: { isRead?: boolean; isStarred?: boolean }) => {
    if (!hasSelection || bulkActionPending) return;
    setBulkActionPending(true);
    void Promise.resolve()
      .then(() => onBulkMutate(selectedMessages, mutation))
      .catch(() => undefined)
      .finally(() => setBulkActionPending(false));
  };

  const runBulkDelete = () => {
    if (!hasSelection || bulkActionPending) return;
    setBulkActionPending(true);
    void Promise.resolve()
      .then(() => onBulkDelete(selectedMessages))
      .then(clearSelection)
      .catch(() => undefined)
      .finally(() => setBulkActionPending(false));
  };

  return (
    <div className="message-list-shell">
      <div className="list-toolbar">
        {bulkMode ? (
          <div className="list-toolbar-status list-selection-summary" aria-live="polite">
            <button
              className="icon-button"
              type="button"
              onClick={exitBulkMode}
              aria-label="退出批量编辑"
              title="退出批量编辑"
            >
              <Icon name="close" size={19} />
            </button>
            <span className="selection-count">
              {hasSelection ? `${selectedMessages.length} 已选` : '批量编辑'}
            </span>
          </div>
        ) : (
          <div className="list-toolbar-status">
            <span className="muted-count">
              {unreadCount > 0 ? `${unreadCount} 封未读` : '全部已读'}
            </span>
            <span className="list-total-count">共 {messages.length} 封</span>
          </div>
        )}
        <div className="list-toolbar-actions">
          {bulkMode && hasSelection ? (
            <>
              <button
                className="icon-button bulk-action"
                type="button"
                onClick={() => runBulkMutation({ isRead: true })}
                disabled={bulkActionPending}
                aria-label="标记为已读"
                title="标记为已读"
              >
                <Icon name="markRead" size={19} />
              </button>
              <button
                className="icon-button bulk-action"
                type="button"
                onClick={() => runBulkMutation({ isRead: false })}
                disabled={bulkActionPending}
                aria-label="标记为未读"
                title="标记为未读"
              >
                <Icon name="markUnread" size={19} />
              </button>
              <button
                className="icon-button bulk-action bulk-delete"
                type="button"
                onClick={runBulkDelete}
                disabled={bulkActionPending}
                aria-label="移至回收站"
                title="移至回收站"
              >
                <Icon name="trash" size={19} />
              </button>
            </>
          ) : bulkMode ? (
            <button
              className="icon-button list-select-all"
              type="button"
              onClick={selectAll}
              disabled={messages.length === 0 || isLoading}
              aria-label="全选当前列表"
              title="全选当前列表"
            >
              <Icon name="selectAll" size={19} />
            </button>
          ) : (
            <>
              <button
                className="icon-button"
                type="button"
                onClick={onRefresh}
                aria-label="刷新邮件"
                title="刷新同步"
              >
                <Icon name="refresh" size={19} />
              </button>
              <button
                className="text-action list-bulk-mode-trigger"
                type="button"
                onClick={enterBulkMode}
                disabled={messages.length === 0 || isLoading}
                aria-label="批量编辑"
                title="批量编辑"
              >
                <Icon name="selectAll" size={19} />
                <span>批量编辑</span>
              </button>
            </>
          )}
        </div>
      </div>
      <div className="search-bar" role="search" onClick={openSearch}>
        <Icon name="search" size={18} />
        <input
          aria-label="搜索已缓存邮件"
          placeholder="搜索邮件、发件人或主题…"
          readOnly
          onFocus={openSearch}
        />
        <kbd className="search-shortcut">⌘ K</kbd>
      </div>
      <div ref={parentRef} className="virtual-list" role="list" aria-label="已缓存邮件">
        {isLoading ? (
          <div className="list-loading">
            <span className="spinner" />
            <span>正在读取本地邮件…</span>
          </div>
        ) : messages.length === 0 ? (
          <div className="list-loading">
            <Icon name="inbox" size={32} />
            <p style={{ margin: '12px 0 4px', fontWeight: 500 }}>没有匹配的邮件</p>
            <span style={{ fontSize: '12px', opacity: 0.7 }}>当前视图下暂无邮件内容</span>
          </div>
        ) : (
          <div style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative' }}>
            {virtualizer.getVirtualItems().map((item) => {
              const message = messages[item.index];
              return (
                <MessageRow
                  key={message.id}
                  message={message}
                  accountLabel={
                    accounts.length > 1
                      ? accounts.find((account) => account.id === message.accountId)?.email
                      : undefined
                  }
                  selected={message.id === selectedMessageId}
                  bulkMode={bulkMode}
                  multiSelected={selectedMessageKeys.has(messageInstanceKey(message))}
                  onSelect={onSelect}
                  onToggle={(mutation) => onToggle(message.id, mutation)}
                  onToggleSelection={() => toggleSelection(message)}
                  style={{ transform: `translateY(${item.start}px)` }}
                />
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

function MessageRow({
  message,
  accountLabel,
  selected,
  bulkMode,
  multiSelected,
  onSelect,
  onToggle,
  onToggleSelection,
  style,
}: {
  message: Message;
  accountLabel?: string;
  selected: boolean;
  bulkMode: boolean;
  multiSelected: boolean;
  onSelect: (id: string) => void;
  onToggle: (mutation: { isRead?: boolean; isStarred?: boolean }) => void;
  onToggleSelection: () => void;
  style: CSSProperties;
}) {
  const avatarLetter =
    message.from.name?.slice(0, 1) ?? message.from.email.slice(0, 1).toUpperCase();

  return (
    <article
      className={`message-row ${selected ? 'is-selected' : ''} ${multiSelected ? 'is-multi-selected' : ''} ${message.isRead ? '' : 'is-unread'}`}
      style={style}
      role="listitem"
      onClick={() => onSelect(message.id)}
    >
      {bulkMode && (
        <button
          className={`message-select-control ${multiSelected ? 'is-checked' : ''} selection-active`}
          type="button"
          aria-label={
            multiSelected
              ? `取消选择 ${message.subject || '(无主题)'}`
              : `选择 ${message.subject || '(无主题)'}`
          }
          aria-pressed={multiSelected}
          onClick={(event) => {
            event.stopPropagation();
            onToggleSelection();
          }}
        >
          <Icon name="check" size={17} />
        </button>
      )}
      <div className="row-avatar">{avatarLetter}</div>
      <div className="row-main">
        <div className="row-topline">
          <span className="sender-name">{message.from.name ?? message.from.email}</span>
          <span className="row-time">{formatTime(message.date)}</span>
        </div>
        <div className="row-subject">
          <span className="subject-text">{message.subject || '(无主题)'}</span>
          {!message.isRead && <span className="unread-dot" title="未读" />}
        </div>
        <div className="row-preview">
          {accountLabel && <span className="row-account-label">{accountLabel}</span>}
          {message.preview}
        </div>
      </div>
      <div className="row-actions">
        <button
          className={`row-icon ${message.isStarred ? 'is-starred' : ''}`}
          aria-label={message.isStarred ? '取消星标' : '加星标'}
          title={message.isStarred ? '取消星标' : '标记为星标'}
          onClick={(event) => {
            event.stopPropagation();
            onToggle({ isStarred: !message.isStarred });
          }}
        >
          <Icon name={message.isStarred ? 'starFilled' : 'star'} size={18} />
        </button>
        {message.hasAttachment && (
          <Icon name="paperclip" size={16} className="attachment-icon" title="包含附件" />
        )}
        <button
          className="row-icon row-read-toggle"
          aria-label={message.isRead ? '标记为未读' : '标记为已读'}
          title={message.isRead ? '标记为未读' : '标记为已读'}
          onClick={(event) => {
            event.stopPropagation();
            onToggle({ isRead: !message.isRead });
          }}
        >
          <Icon name={message.isRead ? 'check' : 'clock'} size={16} />
        </button>
      </div>
    </article>
  );
}
