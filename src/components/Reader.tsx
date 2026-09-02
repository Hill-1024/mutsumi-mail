import type { Message } from '../types';
import { Icon } from '../lib/icons';
import { useUiStore } from '../stores/ui';

export function Reader({ message, onBack, onMutate, onArchive, onDelete }: { message: Message; onBack?: () => void; onMutate?: (messageId: string, mutation: { isRead?: boolean; isStarred?: boolean }) => void; onArchive?: (messageId: string) => void; onDelete?: (messageId: string) => void }) {
  const { setComposeOpen } = useUiStore();
  const initials = message.from.name?.split(' ').map((part) => part[0]).join('').slice(0, 2) ?? message.from.email.slice(0, 2).toUpperCase();
  return (
    <article className="reader">
      <div className="reader-toolbar"><button className="reader-tool" onClick={onBack} aria-label="返回列表"><Icon name="back" size={19} /></button><span className="reader-toolbar-label">邮件详情</span><span className="reader-toolbar-spacer" /><button className="reader-tool" onClick={() => onArchive?.(message.id)} aria-label="归档"><Icon name="archive" size={19} /></button><button className="reader-tool" onClick={() => onDelete?.(message.id)} aria-label="删除"><Icon name="trash" size={18} /></button><button className="reader-tool" aria-label="更多"><Icon name="more" size={19} /></button></div>
      <div className="reader-content">
        <div className="reader-label-row"><span className="label-chip">设计</span><span className="label-chip">收件箱</span><span className="reader-date">2026 年 9 月 2 日 08:31</span></div>
        <h2 className="reader-subject">{message.subject}</h2>
        <div className="sender-block"><div className="sender-avatar">{initials}</div><div className="sender-details"><strong>{message.from.name ?? message.from.email}</strong><span>{message.from.email}</span></div><button className={`row-icon reader-star ${message.isStarred ? 'is-starred' : ''}`} onClick={() => onMutate?.(message.id, { isStarred: !message.isStarred })} aria-label="切换星标"><Icon name="star" size={19} /></button><button className="icon-button tiny" aria-label="更多发件人操作"><Icon name="more" size={18} /></button></div>
        <div className="safe-html-notice"><Icon name="shield" size={17} /><span>已启用安全阅读模式。远程图片和脚本已阻止。</span><button>查看原始邮件</button></div>
        <div className="reader-body">{(message.bodyText ?? message.preview).split('\n').map((paragraph, index) => paragraph ? <p key={`${message.id}-${index}`}>{paragraph}</p> : <div className="body-break" key={`${message.id}-${index}`} />)}</div>
        {message.hasAttachment && <div className="attachment-card"><div className="attachment-leading"><Icon name="paperclip" size={19} /></div><div className="attachment-copy"><strong>design-review-notes.pdf</strong><span>PDF · 2.4 MB · 已缓存</span></div><button className="icon-button" aria-label="下载附件"><Icon name="download" size={18} /></button></div>}
        <div className="reader-actions"><button className="outlined-action" onClick={() => setComposeOpen(true)}><Icon name="reply" size={18} />回复</button><button className="outlined-action" onClick={() => setComposeOpen(true)}><Icon name="replyAll" size={18} />回复全部</button><button className="outlined-action" onClick={() => setComposeOpen(true)}><Icon name="forward" size={18} />转发</button></div>
      </div>
    </article>
  );
}
