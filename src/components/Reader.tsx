import type { Message } from '../types';
import { Icon } from '../lib/icons';
import { useUiStore } from '../stores/ui';

interface ReaderProps {
  message: Message;
  accountEmail?: string;
  bodyLoading?: boolean;
  bodyError?: string;
  onRetryBody?: () => void;
  onBack?: () => void;
  onMutate?: (messageId: string, mutation: { isRead?: boolean; isStarred?: boolean }) => void;
  onArchive?: (messageId: string) => void;
  onDelete?: (messageId: string) => void;
}

const formatFullDate = (value: string) => {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
};

export function Reader({ message, accountEmail, bodyLoading, bodyError, onRetryBody, onBack, onMutate, onArchive, onDelete }: ReaderProps) {
  const { openComposeWithDraft, safeReading } = useUiStore();
  const initials = message.from.name
    ? message.from.name.split(' ').map((part) => part[0]).join('').slice(0, 2).toUpperCase()
    : message.from.email.slice(0, 2).toUpperCase();
  const readableBody = message.bodyText ?? message.bodyHtmlText ?? message.preview;

  const handleReply = () => {
    openComposeWithDraft({
      accountId: message.accountId,
      to: message.from.email,
      subject: message.subject.toLowerCase().startsWith('re:') ? message.subject : `Re: ${message.subject}`,
      bodyText: `\n\n\n--- 原始邮件 ---\n发件人: ${message.from.name ?? ''} <${message.from.email}>\n日期: ${formatFullDate(message.date)}\n主题: ${message.subject}\n\n${readableBody}`,
      inReplyTo: message.messageId,
      references: message.messageId ? [message.messageId] : undefined,
    });
  };

  const handleReplyAll = () => {
    const ccList = (message.to ?? [])
      .map((t) => t.email)
      .filter((email) =>
        email &&
        email.toLowerCase() !== message.from.email.toLowerCase() &&
        (!accountEmail || email.toLowerCase() !== accountEmail.toLowerCase()),
      )
      .join(', ');

    openComposeWithDraft({
      accountId: message.accountId,
      to: message.from.email,
      cc: ccList || undefined,
      subject: message.subject.toLowerCase().startsWith('re:') ? message.subject : `Re: ${message.subject}`,
      bodyText: `\n\n\n--- 原始邮件 ---\n发件人: ${message.from.name ?? ''} <${message.from.email}>\n日期: ${formatFullDate(message.date)}\n主题: ${message.subject}\n\n${readableBody}`,
      inReplyTo: message.messageId,
      references: message.messageId ? [message.messageId] : undefined,
    });
  };

  const handleForward = () => {
    openComposeWithDraft({
      accountId: message.accountId,
      to: '',
      subject: message.subject.toLowerCase().startsWith('fwd:') ? message.subject : `Fwd: ${message.subject}`,
      bodyText: `\n\n\n--- 转发邮件 ---\n发件人: ${message.from.name ?? ''} <${message.from.email}>\n日期: ${formatFullDate(message.date)}\n主题: ${message.subject}\n\n${readableBody}`,
    });
  };

  return (
    <article className="reader">
      <div className="reader-toolbar">
        <button className="reader-tool reader-back" onClick={onBack} aria-label="返回列表" title="返回列表 (Esc)">
          <Icon name="back" size={20} />
        </button>
        <span className="reader-toolbar-spacer" />
        <button
          className={`reader-tool ${message.isStarred ? 'is-starred' : ''}`}
          onClick={() => onMutate?.(message.id, { isStarred: !message.isStarred })}
          aria-label={message.isStarred ? '取消星标' : '添加星标'}
          title={message.isStarred ? '取消星标' : '标记为星标'}
        >
          <Icon name={message.isStarred ? 'starFilled' : 'star'} size={20} />
        </button>
        <button className="reader-tool" onClick={() => onArchive?.(message.id)} aria-label="归档" title="归档邮件">
          <Icon name="archive" size={20} />
        </button>
        <button className="reader-tool" onClick={() => onDelete?.(message.id)} aria-label="删除" title="移至回收站">
          <Icon name="trash" size={20} />
        </button>
      </div>

      <div className="reader-content">
        <h2 className="reader-subject">{message.subject || '(无主题)'}</h2>

        <div className="sender-block">
          <div className="sender-avatar">{initials}</div>
          <div className="sender-details">
            <div className="sender-name-row">
              <strong className="sender-title">{message.from.name ?? message.from.email}</strong>
              <span className="sender-email">&lt;{message.from.email}&gt;</span>
            </div>
            <span className="sender-date">{formatFullDate(message.date)}</span>
          </div>
          {message.labels && message.labels.length > 0 && (
            <div className="reader-labels">
              {message.labels.map((label) => (
                <span key={label} className="reader-label-chip">#{label}</span>
              ))}
            </div>
          )}
        </div>

        {safeReading && (
          <div className="reader-safety" title="远程图片与外部脚本已拦截">
            <Icon name="shield" size={16} />
            <span>已阻止远程内容</span>
          </div>
        )}

        {bodyLoading && <div className="reader-body-status" role="status"><span className="spinner" />正在下载正文…</div>}
        {!bodyLoading && bodyError && (
          <div className="reader-body-status is-error" role="alert">
            <span>{bodyError}</span>
            <button className="text-action" type="button" onClick={onRetryBody}>重试</button>
          </div>
        )}

        <div className="reader-body">
          {readableBody.split('\n').map((paragraph, index) =>
            paragraph.trim() ? (
              <p key={`${message.id}-${index}`}>{paragraph}</p>
            ) : (
              <div className="body-break" key={`${message.id}-${index}`} />
            )
          )}
        </div>

        {message.hasAttachment && (
          <div className="attachment-card" role="status" aria-label="邮件包含附件">
            <div className="attachment-icon-wrapper">
              <Icon name="paperclip" size={20} />
            </div>
            <div className="attachment-copy">
              <strong>{message.attachmentCount ? `包含 ${message.attachmentCount} 个附件` : '包含附件'}</strong>
            </div>
          </div>
        )}

        <div className="reader-actions" aria-label="快捷操作">
          <button className="outlined-action" onClick={handleReply} title="回复此邮件">
            <Icon name="reply" size={18} />
            <span>回复</span>
          </button>
          <button className="outlined-action" onClick={handleReplyAll} title="回复所有收件人">
            <Icon name="replyAll" size={18} />
            <span>回复全部</span>
          </button>
          <button className="outlined-action" onClick={handleForward} title="转发此邮件">
            <Icon name="forward" size={18} />
            <span>转发</span>
          </button>
        </div>
      </div>
    </article>
  );
}
