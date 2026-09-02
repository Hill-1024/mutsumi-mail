import { useRef, type CSSProperties } from 'react';
import { useNavigate } from 'react-router-dom';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { Message } from '../types';
import { Icon } from '../lib/icons';
import { useUiStore } from '../stores/ui';

interface MessageListProps {
  messages: Message[];
  selectedMessageId?: string;
  onSelect: (id: string) => void;
  onToggle: (messageId: string, mutation: { isRead?: boolean; isStarred?: boolean }) => void;
  onRefresh?: () => void;
  isLoading?: boolean;
}

const formatTime = (value: string) => {
  const date = new Date(value);
  const now = new Date('2026-09-02T12:00:00+08:00');
  return date.toDateString() === now.toDateString()
    ? date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
    : date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
};

export function MessageList({ messages, selectedMessageId, onSelect, onToggle, onRefresh, isLoading }: MessageListProps) {
  const parentRef = useRef<HTMLDivElement>(null);
  const { setSearchOpen, setNavPage } = useUiStore();
  const navigate = useNavigate();
  const openSearch = () => { setSearchOpen(true); setNavPage('search'); navigate('/search'); };
  const virtualizer = useVirtualizer({ count: messages.length, getScrollElement: () => parentRef.current, estimateSize: () => 92, overscan: 8 });

  return (
    <div className="message-list-shell">
      <div className="list-toolbar">
        <div><h1>收件箱</h1><span className="muted-count">{messages.filter((message) => !message.isRead).length} 封未读</span></div>
        <div className="list-toolbar-actions"><button className="icon-button" onClick={openSearch} aria-label="搜索邮件" title="搜索邮件"><Icon name="search" size={20} /></button><button className="icon-button" onClick={onRefresh} aria-label="刷新同步" title="刷新同步"><Icon name="refresh" size={19} /></button></div>
      </div>
      <div className="search-bar" role="search"><Icon name="search" size={17} /><input aria-label="搜索已缓存邮件" placeholder="搜索邮件、发件人或主题" onFocus={openSearch} /><kbd>⌘ K</kbd></div>
      <div className="list-filter-row"><button className="filter-chip is-selected">全部邮件</button><button className="filter-chip">未读</button><button className="filter-chip">有附件</button><span className="filter-spacer" /><button className="icon-button tiny" aria-label="排序"><Icon name="more" size={17} /></button></div>
      <div ref={parentRef} className="virtual-list" role="list" aria-label="已缓存邮件">
        {isLoading ? <div className="list-loading"><span className="spinner" />正在读取本地邮件…</div> : messages.length === 0 ? <div className="list-loading">没有匹配的邮件</div> : <div style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative' }}>
          {virtualizer.getVirtualItems().map((item) => {
            const message = messages[item.index];
            return <MessageRow key={message.id} message={message} selected={message.id === selectedMessageId} onSelect={onSelect} onToggle={(mutation) => onToggle(message.id, mutation)} style={{ transform: `translateY(${item.start}px)` }} />;
          })}
        </div>}
      </div>
      <div className="list-footer"><span><span className="small-green-dot" />仅显示已缓存内容</span><span>{messages.length} / 200</span></div>
    </div>
  );
}

function MessageRow({ message, selected, onSelect, onToggle, style }: { message: Message; selected: boolean; onSelect: (id: string) => void; onToggle: (mutation: { isRead?: boolean; isStarred?: boolean }) => void; style: CSSProperties }) {
  return (
    <article className={`message-row ${selected ? 'is-selected' : ''} ${message.isRead ? '' : 'is-unread'}`} style={style} role="listitem" onClick={() => onSelect(message.id)}>
      <div className="row-avatar">{message.from.name?.slice(0, 1) ?? message.from.email.slice(0, 1).toUpperCase()}</div>
      <div className="row-main">
        <div className="row-topline"><span className="sender-name">{message.from.name ?? message.from.email}</span><span className="row-time">{formatTime(message.date)}</span></div>
        <div className="row-subject"><span>{message.subject}</span>{!message.isRead && <span className="unread-dot" />}</div>
        <div className="row-preview">{message.preview}</div>
      </div>
      <div className="row-actions"><button className={`row-icon ${message.isStarred ? 'is-starred' : ''}`} aria-label={message.isStarred ? '取消星标' : '加星'} onClick={(event) => { event.stopPropagation(); onToggle({ isStarred: !message.isStarred }); }}><Icon name="star" size={17} /></button>{message.hasAttachment && <Icon name="paperclip" size={16} className="attachment-icon" />}<button className="row-icon row-read-toggle" aria-label={message.isRead ? '标记未读' : '标记已读'} onClick={(event) => { event.stopPropagation(); onToggle({ isRead: !message.isRead }); }}><Icon name={message.isRead ? 'check' : 'clock'} size={15} /></button></div>
    </article>
  );
}
