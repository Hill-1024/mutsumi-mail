import { useEffect, useState } from 'react';
import { useForm } from 'react-hook-form';
import { useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { zodResolver } from '@hookform/resolvers/zod';
import { Icon } from '../lib/icons';
import { appErrorMessage, saveDraft, sendDraft } from '../lib/tauri';
import type { OutboxItem } from '../types';

const composeSchema = z.object({
  to: z.string().min(3, '请至少填写一个收件人'),
  cc: z.string().optional(),
  bcc: z.string().optional(),
  subject: z.string().min(1, '请输入主题'),
  bodyText: z.string().min(1, '请输入邮件内容'),
});

type ComposeForm = z.infer<typeof composeSchema>;

export function ComposeDialog({ accountId, onClose, onQueued }: { accountId: string; onClose: () => void; onQueued?: (item: OutboxItem) => void }) {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<'idle' | 'saving' | 'queued' | 'error'>('idle');
  const [statusMessage, setStatusMessage] = useState('');
  const { register, handleSubmit, watch, formState: { errors, isDirty } } = useForm<ComposeForm>({ resolver: zodResolver(composeSchema), defaultValues: { to: '', cc: '', bcc: '', subject: '', bodyText: '' } });
  const values = watch();

  useEffect(() => {
    const timer = window.setTimeout(() => {
      if (isDirty && values.bodyText) void saveDraft({ ...values, accountId }).catch(() => undefined);
    }, 900);
    return () => window.clearTimeout(timer);
  }, [accountId, isDirty, values]);

  const submit = async (mode: 'save' | 'send') => {
    setStatus('saving');
    try {
      if (mode === 'save') {
        await saveDraft({ ...values, accountId });
        await queryClient.invalidateQueries({ queryKey: ['outbox', accountId] });
        setStatusMessage('草稿已保存到本地');
        setStatus('queued');
      } else {
        const result = await sendDraft({ ...values, accountId });
        const queuedItem: OutboxItem = { id: result.outboxId, accountId, subject: values.subject, recipients: values.to.split(',').map((recipient) => recipient.trim()).filter(Boolean), state: 'queued', updatedAt: new Date().toISOString() };
        queryClient.setQueryData<OutboxItem[]>(['outbox', accountId], (current = []) => [queuedItem, ...current.filter((item) => item.id !== queuedItem.id)]);
        onQueued?.(queuedItem);
        await queryClient.invalidateQueries({ queryKey: ['outbox', accountId] });
        setStatusMessage(result.state === 'queued' ? '邮件已进入本地队列；网络恢复后可重试' : '邮件已交给发件队列');
        setStatus('queued');
      }
    } catch (error) {
      setStatus('error');
      setStatusMessage(appErrorMessage(error));
    }
  };

  return (
    <div className="modal-scrim" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <section className="compose-dialog" role="dialog" aria-modal="true" aria-labelledby="compose-title">
        <header className="compose-header"><div><span className="compose-kicker">本地草稿</span><h2 id="compose-title">新邮件</h2></div><button className="icon-button" onClick={onClose} aria-label="关闭撰写"><Icon name="close" size={20} /></button></header>
        <form onSubmit={handleSubmit(() => submit('send'))}>
          <label className="field-row"><span>收件人</span><input {...register('to')} placeholder="name@example.com" autoFocus /><button type="button" className="field-action">抄送/密送</button></label>{errors.to && <div className="field-error">{errors.to.message}</div>}
          <label className="field-row"><span>主题</span><input {...register('subject')} placeholder="主题" /></label>{errors.subject && <div className="field-error">{errors.subject.message}</div>}
          <textarea className="compose-body" {...register('bodyText')} placeholder="写下你的想法…" rows={10} />{errors.bodyText && <div className="field-error">{errors.bodyText.message}</div>}
          <div className="compose-toolbar"><button type="button" className="icon-button" aria-label="添加附件"><Icon name="paperclip" size={19} /></button><span className="compose-toolbar-hint">支持纯文本与附件 · 自动保存到本地</span><span className="compose-spacer" /><button type="button" className="text-action" onClick={() => void submit('save')}>保存草稿</button><button className="send-action" type="submit"><Icon name="send" size={17} />发送</button></div>
          {status !== 'idle' && <div className={`compose-status ${status === 'error' ? 'is-error' : ''}`}><Icon name={status === 'error' ? 'close' : 'checkCircle'} size={17} />{statusMessage || '正在保存…'}</div>}
        </form>
      </section>
    </div>
  );
}
