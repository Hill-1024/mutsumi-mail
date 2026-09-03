import { useEffect, useMemo, useState } from 'react';
import { useForm, useWatch } from 'react-hook-form';
import { useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { zodResolver } from '@hookform/resolvers/zod';
import { Icon } from '../lib/icons';
import { appErrorMessage, saveDraft, sendDraft } from '../lib/tauri';
import type { Account, OutboxItem } from '../types';
import { useUiStore } from '../stores/ui';

const composeSchema = z.object({
  to: z.string().min(1, '请至少填写一个收件人'),
  cc: z.string().optional(),
  bcc: z.string().optional(),
  subject: z.string().min(1, '请输入主题'),
  bodyText: z.string().min(1, '请输入邮件内容'),
});

type ComposeForm = z.infer<typeof composeSchema>;

export function ComposeDialog({
  accounts,
  defaultAccountId,
  onClose,
  onQueued,
}: {
  accounts: Account[];
  defaultAccountId?: string | null;
  onClose: () => void;
  onQueued?: (item: OutboxItem) => void;
}) {
  const queryClient = useQueryClient();
  const { composeDraft, clearComposeDraft } = useUiStore();
  const sendAccounts = useMemo(
    () => accounts.filter((account) => account.enabled && account.outgoingConfigured),
    [accounts],
  );
  const preferredAccountId = composeDraft?.accountId ?? defaultAccountId;
  const initialAccountId = sendAccounts.some((account) => account.id === preferredAccountId)
    ? (preferredAccountId ?? '')
    : (sendAccounts[0]?.id ?? '');
  const [accountId, setAccountId] = useState(initialAccountId);
  const [draftId, setDraftId] = useState<string>();
  const [status, setStatus] = useState<'idle' | 'saving' | 'queued' | 'error'>('idle');
  const [statusMessage, setStatusMessage] = useState('');
  const [showCcBcc, setShowCcBcc] = useState(Boolean(composeDraft?.cc || composeDraft?.bcc));
  const selectedSenderId = sendAccounts.some((account) => account.id === accountId)
    ? accountId
    : initialAccountId;

  const {
    register,
    handleSubmit,
    getValues,
    control,
    formState: { errors, isDirty },
  } = useForm<ComposeForm>({
    resolver: zodResolver(composeSchema),
    defaultValues: {
      to: composeDraft?.to ?? '',
      cc: composeDraft?.cc ?? '',
      bcc: composeDraft?.bcc ?? '',
      subject: composeDraft?.subject ?? '',
      bodyText: composeDraft?.bodyText ?? '',
    },
  });
  const watchedValues = useWatch({ control });

  // Handle escape key
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        clearComposeDraft();
        onClose();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [clearComposeDraft, onClose]);

  // Auto-save draft
  useEffect(() => {
    if (status !== 'idle') return undefined;
    const timer = window.setTimeout(() => {
      if (isDirty && selectedSenderId) {
        if (watchedValues.bodyText || watchedValues.subject) {
          void saveDraft({
            id: draftId,
            to: watchedValues.to ?? '',
            cc: watchedValues.cc,
            bcc: watchedValues.bcc,
            subject: watchedValues.subject ?? '',
            bodyText: watchedValues.bodyText ?? '',
            accountId: selectedSenderId,
            inReplyTo: composeDraft?.inReplyTo,
            references: composeDraft?.references,
          })
            .then((saved) => setDraftId((current) => current ?? saved.id))
            .catch(() => undefined);
        }
      }
    }, 1200);
    return () => window.clearTimeout(timer);
  }, [
    composeDraft?.inReplyTo,
    composeDraft?.references,
    draftId,
    isDirty,
    selectedSenderId,
    status,
    watchedValues.bcc,
    watchedValues.bodyText,
    watchedValues.cc,
    watchedValues.subject,
    watchedValues.to,
  ]);

  const submit = async (mode: 'save' | 'send', formData?: ComposeForm) => {
    if (!selectedSenderId) {
      setStatus('error');
      setStatusMessage('没有可用的发件账户，请先在设置中完成发件配置。');
      return;
    }

    const values = formData ?? getValues();
    setStatus('saving');
    setStatusMessage('');
    try {
      if (mode === 'save') {
        const saved = await saveDraft({
          id: draftId,
          to: values.to,
          cc: values.cc,
          bcc: values.bcc,
          subject: values.subject || '(无主题草稿)',
          bodyText: values.bodyText,
          accountId: selectedSenderId,
          inReplyTo: composeDraft?.inReplyTo,
          references: composeDraft?.references,
        });
        setDraftId(saved.id);
        await queryClient.invalidateQueries({ queryKey: ['outbox'] });
        setStatusMessage('草稿已保存在本地');
        setStatus('queued');
      } else {
        const result = await sendDraft({
          id: draftId,
          to: values.to,
          cc: values.cc,
          bcc: values.bcc,
          subject: values.subject,
          bodyText: values.bodyText,
          accountId: selectedSenderId,
          inReplyTo: composeDraft?.inReplyTo,
          references: composeDraft?.references,
        });
        const queuedItem: OutboxItem = {
          id: result.outboxId,
          accountId: selectedSenderId,
          subject: values.subject,
          recipients: values.to
            .split(',')
            .map((r) => r.trim())
            .filter(Boolean),
          state: 'queued',
          updatedAt: new Date().toISOString(),
        };
        queryClient.setQueryData<OutboxItem[]>(['outbox', selectedSenderId], (current = []) => [
          queuedItem,
          ...current.filter((item) => item.id !== queuedItem.id),
        ]);
        onQueued?.(queuedItem);
        await queryClient.invalidateQueries({ queryKey: ['outbox'] });
        setStatusMessage('邮件已加入发件队列');
        setStatus('queued');

        // Smooth auto-close after 1 second
        window.setTimeout(() => {
          clearComposeDraft();
          onClose();
        }, 900);
      }
    } catch (error) {
      setStatus('error');
      setStatusMessage(appErrorMessage(error));
    }
  };

  const handleScrimClose = () => {
    clearComposeDraft();
    onClose();
  };

  return (
    <div
      className="modal-scrim"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) handleScrimClose();
      }}
    >
      <section
        className="compose-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="compose-title"
      >
        <header className="compose-header">
          <h2 id="compose-title">{composeDraft?.subject ? '回复 / 转发邮件' : '新邮件'}</h2>
          <button
            className="icon-button"
            onClick={handleScrimClose}
            aria-label="关闭撰写"
            title="关闭 (Esc)"
          >
            <Icon name="close" size={20} />
          </button>
        </header>

        <form onSubmit={handleSubmit((data) => submit('send', data))}>
          <label className="field-row compose-account-field">
            <span className="field-label">发件账户</span>
            <select
              className="compose-account-select"
              value={selectedSenderId}
              disabled={sendAccounts.length === 0 || status === 'saving'}
              onChange={(event) => {
                setAccountId(event.target.value);
                setDraftId(undefined);
                setStatus('idle');
                setStatusMessage('');
              }}
              aria-label="选择发件账户"
            >
              {sendAccounts.length === 0 ? <option value="">没有可用的发件账户</option> : null}
              {sendAccounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.displayName ? `${account.displayName} · ` : ''}
                  {account.email}
                </option>
              ))}
            </select>
          </label>
          {sendAccounts.length === 0 ? (
            <div className="compose-account-error" role="alert">
              请先在设置中添加并验证可发件的邮箱账户。
            </div>
          ) : null}

          <label className="field-row">
            <span className="field-label">收件人</span>
            <input
              {...register('to')}
              placeholder="输入一个或多个邮箱地址"
              autoFocus
              autoComplete="email"
            />
            <button
              type="button"
              className={`field-action ${showCcBcc ? 'is-active' : ''}`}
              onClick={() => setShowCcBcc((prev) => !prev)}
            >
              {showCcBcc ? '隐藏抄送' : '抄送/密送'}
            </button>
          </label>
          {errors.to && <div className="field-error">{errors.to.message}</div>}

          {showCcBcc && (
            <>
              <label className="field-row">
                <span className="field-label">抄送</span>
                <input {...register('cc')} placeholder="输入抄送邮箱地址" />
              </label>
              <label className="field-row">
                <span className="field-label">密送</span>
                <input {...register('bcc')} placeholder="输入密送邮箱地址" />
              </label>
            </>
          )}

          <label className="field-row">
            <span className="field-label">主题</span>
            <input {...register('subject')} placeholder="邮件主题" />
          </label>
          {errors.subject && <div className="field-error">{errors.subject.message}</div>}

          <textarea
            className="compose-body"
            {...register('bodyText')}
            placeholder="写下你的邮件内容…"
            rows={10}
          />
          {errors.bodyText && <div className="field-error">{errors.bodyText.message}</div>}

          {status !== 'idle' && (
            <div
              className={`compose-status ${status === 'error' ? 'is-error' : ''}`}
              role={status === 'error' ? 'alert' : 'status'}
              aria-live="polite"
            >
              <Icon name={status === 'error' ? 'close' : 'checkCircle'} size={18} />
              <span>{statusMessage || '正在保存…'}</span>
            </div>
          )}

          <div className="compose-toolbar">
            <span className="compose-spacer" />
            <button
              type="button"
              className="text-action"
              onClick={() => void submit('save')}
              disabled={!selectedSenderId || status === 'saving'}
              title="保存为草稿"
            >
              保存草稿
            </button>
            <button
              className="send-action"
              type="submit"
              disabled={!selectedSenderId || status === 'saving'}
              title="发送邮件"
            >
              <Icon name="send" size={18} />
              <span>发送</span>
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}
