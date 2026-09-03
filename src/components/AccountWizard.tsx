import { useEffect, useMemo, useState } from 'react';
import { useForm, useWatch } from 'react-hook-form';
import { z } from 'zod';
import { zodResolver } from '@hookform/resolvers/zod';
import { Icon } from '../lib/icons';
import { appErrorMessage, createAccount, providerPresets } from '../lib/tauri';
import type { Account, AppErrorDto, ProviderPreset } from '../types';

const accountSchema = z.object({
  email: z.string().trim().email('请输入有效的邮箱地址'),
  displayName: z.string().trim().optional(),
  secret: z.string().min(1, '请输入客户端授权码或安全凭据'),
  outgoingSecret: z.string().optional(),
  incomingHost: z.string().optional(),
  incomingPort: z.string().optional(),
  incomingUsername: z.string().optional(),
  incomingTlsMode: z.enum(['implicit', 'starttls']),
  outgoingHost: z.string().optional(),
  outgoingPort: z.string().optional(),
  outgoingUsername: z.string().optional(),
  outgoingTlsMode: z.enum(['implicit', 'starttls']),
});

type AccountForm = z.infer<typeof accountSchema>;
type SubmitState = 'idle' | 'verifying' | 'error';

const availableProviderIds = new Set([
  'qq',
  'netease-163',
  'generic',
  'generic-smtp',
  'cloudflare-smtp',
]);

const availableProviders = providerPresets.filter((preset) => availableProviderIds.has(preset.id));

const isOutboundOnlyProvider = (provider: ProviderPreset) => !provider.incoming && Boolean(provider.outgoing);

const providerIcon = (provider: ProviderPreset) => {
  if (provider.id === 'generic') return 'settings' as const;
  return isOutboundOnlyProvider(provider) ? 'send' as const : 'inbox' as const;
};

const providerChoiceDescription = (provider: ProviderPreset) => {
  if (provider.id === 'generic') return '手动配置 IMAP 与 SMTP';
  if (provider.id === 'generic-smtp') return '手动配置 SMTP，仅用于发件';
  if (provider.id === 'cloudflare-smtp') return '固定 SMTP 端点，仅用于发件';
  return '使用客户端授权码登录';
};

const providerDescription = (provider: ProviderPreset) => {
  if (provider.id === 'generic') return 'IMAP 收件 + SMTP 发件';
  return isOutboundOnlyProvider(provider) ? '仅 SMTP 发件，不收取或同步邮件' : '已选择邮箱服务';
};

const errorCode = (error: unknown) => (
  typeof error === 'object' && error && 'code' in error
    ? String((error as AppErrorDto).code)
    : ''
);

const parsePort = (value: string | undefined) => {
  const port = Number(value);
  return Number.isInteger(port) && port >= 1 && port <= 65_535 ? port : null;
};

export function AccountWizard({
  onClose,
  onSaved,
  canClose = true,
}: {
  onClose: () => void;
  onSaved: (account: Account) => void;
  canClose?: boolean;
}) {
  const [step, setStep] = useState<1 | 2>(1);
  const [provider, setProvider] = useState<ProviderPreset | null>(null);
  const [submitState, setSubmitState] = useState<SubmitState>('idle');
  const [status, setStatus] = useState('');
  const {
    register,
    handleSubmit,
    control,
    setError,
    clearErrors,
    setFocus,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<AccountForm>({
    resolver: zodResolver(accountSchema),
    defaultValues: {
      email: '',
      displayName: '',
      secret: '',
      outgoingSecret: '',
      incomingHost: '',
      incomingPort: '993',
      incomingUsername: '',
      incomingTlsMode: 'implicit',
      outgoingHost: '',
      outgoingPort: '465',
      outgoingUsername: '',
      outgoingTlsMode: 'implicit',
    },
  });
  const emailValue = useWatch({ control, name: 'email' }) ?? '';
  const defaultDisplayName = useMemo(() => emailValue.trim().split('@')[0] ?? '', [emailValue]);
  const isVerifying = submitState === 'verifying' || isSubmitting;

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (canClose && !isVerifying && event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [canClose, isVerifying, onClose]);

  const selectProvider = (selectedProvider: ProviderPreset) => {
    reset();
    setProvider(selectedProvider);
    setSubmitState('idle');
    setStatus('');
    clearErrors();
    setStep(2);
  };

  const returnToProviderSelection = () => {
    if (isVerifying) return;
    setSubmitState('idle');
    setStatus('');
    clearErrors();
    setStep(1);
  };

  const save = async (values: AccountForm) => {
    if (!provider) {
      setStep(1);
      setSubmitState('error');
      setStatus('请先选择邮箱服务或协议。');
      return;
    }

    const email = values.email.trim();
    const secret = values.secret;
    const isGenericMailbox = provider.id === 'generic';
    const isGenericSmtp = provider.id === 'generic-smtp';
    const hasManualOutgoing = isGenericMailbox || isGenericSmtp;
    const isOutboundOnly = isOutboundOnlyProvider(provider);
    const incomingHost = values.incomingHost?.trim() ?? '';
    const outgoingHost = values.outgoingHost?.trim() ?? '';
    const incomingPort = parsePort(values.incomingPort);
    const outgoingPort = parsePort(values.outgoingPort);
    let hasEndpointError = false;

    clearErrors();
    if (isGenericMailbox && !incomingHost) {
      setError('incomingHost', { type: 'manual', message: '请输入 IMAP 服务器地址' });
      hasEndpointError = true;
    }
    if (isGenericMailbox && incomingPort === null) {
      setError('incomingPort', { type: 'manual', message: '请输入 1–65535 之间的端口' });
      hasEndpointError = true;
    }
    if (hasManualOutgoing && !outgoingHost) {
      setError('outgoingHost', { type: 'manual', message: '请输入 SMTP 服务器地址' });
      hasEndpointError = true;
    }
    if (hasManualOutgoing && outgoingPort === null) {
      setError('outgoingPort', { type: 'manual', message: '请输入 1–65535 之间的端口' });
      hasEndpointError = true;
    }
    if (hasEndpointError) {
      setSubmitState('error');
      setStatus('请检查标出的服务器配置。');
      return;
    }

    setSubmitState('verifying');
    setStatus(isOutboundOnly ? '正在验证 SMTP 发件连接…' : '正在验证收件与发件连接…');
    let account: Account;
    try {
      account = await createAccount({
        email,
        displayName: values.displayName?.trim() || defaultDisplayName || email,
        providerId: provider.id,
        secret,
        incomingSecret: isGenericMailbox ? secret : undefined,
        outgoingSecret: isGenericMailbox
          ? values.outgoingSecret || secret
          : isGenericSmtp
            ? secret
            : undefined,
        incoming: isGenericMailbox && incomingPort !== null ? {
          protocol: 'imap',
          host: incomingHost,
          port: incomingPort,
          tlsMode: values.incomingTlsMode,
          authMethod: 'password',
          username: values.incomingUsername?.trim() || email,
        } : undefined,
        outgoing: hasManualOutgoing && outgoingPort !== null ? {
          protocol: 'smtp',
          host: outgoingHost,
          port: outgoingPort,
          tlsMode: values.outgoingTlsMode,
          authMethod: 'password',
          username: values.outgoingUsername?.trim() || email,
        } : undefined,
      });
    } catch (error) {
      const message = appErrorMessage(error);
      setStep(2);
      setSubmitState('error');
      setStatus(message);
      if (errorCode(error) === 'authentication') {
        setError('secret', { type: 'server', message });
        window.requestAnimationFrame(() => setFocus('secret'));
      }
      return;
    }
    onSaved(account);
  };

  const reportVisibleValidationErrors = () => {
    setSubmitState('error');
    setStatus('请检查标出的内容后重试。');
  };

  return (
    <div
      className="modal-scrim"
      role="presentation"
      onMouseDown={(event) => {
        if (canClose && !isVerifying && event.target === event.currentTarget) onClose();
      }}
    >
      <section className="wizard-dialog" role="dialog" aria-modal="true" aria-labelledby="wizard-title" aria-busy={isVerifying}>
        <header className="wizard-header">
          <h2 id="wizard-title">添加邮箱</h2>
          {canClose && (
            <button className="icon-button" type="button" disabled={isVerifying} onClick={onClose} aria-label="关闭">
              <Icon name="close" size={20} />
            </button>
          )}
        </header>

        <div className="stepper" aria-label={`添加邮箱，第 ${step} 步，共 2 步`}>
          <span className={`step ${step === 1 ? 'is-current' : 'is-done'}`}>1</span>
          <i />
          <span className={`step ${step === 2 ? 'is-current' : ''}`}>2</span>
        </div>

        {step === 1 && (
          <div className="wizard-step">
            <h3>选择邮箱服务</h3>
            <p className="helper-text">需要收发请选择 IMAP + SMTP；只发信请选择通用 SMTP。</p>
            <div role="group" aria-label="可用邮箱服务">
              {availableProviders.map((preset) => (
                <button
                  key={preset.id}
                  className="provider-detected provider-choice"
                  type="button"
                  onClick={() => selectProvider(preset)}
                >
                  <span className="provider-logo"><Icon name={providerIcon(preset)} size={20} /></span>
                  <span className="provider-choice-copy">
                    <strong>{preset.displayName}</strong>
                    <span>{providerChoiceDescription(preset)}</span>
                  </span>
                  <Icon name="chevron" size={20} />
                </button>
              ))}
            </div>
            {submitState === 'error' && status && <div className="wizard-status is-error" role="alert"><Icon name="close" size={17} />{status}</div>}
          </div>
        )}

        {step === 2 && provider && (
          <form className="wizard-step" onSubmit={handleSubmit(save, reportVisibleValidationErrors)} autoComplete="off" noValidate>
            <div className="provider-detected">
              <div className="provider-logo"><Icon name={providerIcon(provider)} size={20} /></div>
              <div>
                <strong>{provider.displayName}</strong>
                <span>{providerDescription(provider)}</span>
              </div>
              <Icon name="checkCircle" size={22} />
            </div>

            <div className="help-callout"><Icon name="shield" size={18} /><p>{provider.helpText}</p></div>

            {status && (
              <div className={`wizard-status ${submitState === 'error' ? 'is-error' : ''}`} role={submitState === 'error' ? 'alert' : 'status'} aria-live="polite">
                {isVerifying ? <span className="spinner" /> : <Icon name="close" size={17} />}
                <span>{status}</span>
              </div>
            )}

            <label className="form-field">
              <span>邮箱地址</span>
              <input {...register('email')} type="email" autoComplete="off" placeholder="name@example.com" disabled={isVerifying} autoFocus aria-invalid={Boolean(errors.email)} />
              {errors.email && <em>{errors.email.message}</em>}
            </label>
            <label className="form-field">
              <span>显示名称（可选）</span>
              <input {...register('displayName')} autoComplete="off" placeholder={defaultDisplayName ? `默认使用 ${defaultDisplayName}` : '默认使用邮箱名称'} disabled={isVerifying} aria-invalid={Boolean(errors.displayName)} />
              {errors.displayName && <em>{errors.displayName.message}</em>}
            </label>
            <label className="form-field">
              <span>{provider.id === 'qq' || provider.id === 'netease-163'
                ? '客户端授权码'
                : provider.id === 'generic-smtp'
                  ? 'SMTP 密码或授权码'
                  : provider.id === 'cloudflare-smtp'
                    ? 'Cloudflare API Token'
                    : '密码或授权码'}</span>
              <input {...register('secret')} type="password" autoComplete="new-password" placeholder="仅保存到系统安全存储" disabled={isVerifying} aria-invalid={Boolean(errors.secret)} />
              {errors.secret && <em>{errors.secret.message}</em>}
            </label>

            {provider.id === 'generic' && (
              <label className="form-field">
                <span>SMTP 密码或授权码（可选）</span>
                <input {...register('outgoingSecret')} type="password" autoComplete="new-password" placeholder="留空则与收件凭据相同" disabled={isVerifying} />
              </label>
            )}

            {(provider.id === 'generic' || provider.id === 'generic-smtp') && (
              <div className="endpoint-grid">
                {provider.id === 'generic' && (
                  <>
                    <div className="endpoint-heading">收件 IMAP</div>
                    <label className="form-field">
                      <span>服务器</span>
                      <input {...register('incomingHost')} autoComplete="off" placeholder="imap.example.com" disabled={isVerifying} aria-invalid={Boolean(errors.incomingHost)} />
                      {errors.incomingHost && <em>{errors.incomingHost.message}</em>}
                    </label>
                    <label className="form-field">
                      <span>端口</span>
                      <input {...register('incomingPort')} inputMode="numeric" disabled={isVerifying} aria-invalid={Boolean(errors.incomingPort)} />
                      {errors.incomingPort && <em>{errors.incomingPort.message}</em>}
                    </label>
                    <label className="form-field">
                      <span>用户名（可选）</span>
                      <input {...register('incomingUsername')} autoComplete="off" placeholder="默认使用邮箱地址" disabled={isVerifying} />
                    </label>
                    <label className="form-field">
                      <span>TLS 模式</span>
                      <select {...register('incomingTlsMode')} disabled={isVerifying}>
                        <option value="implicit">IMAPS（通常 993）</option>
                        <option value="starttls">STARTTLS（通常 143）</option>
                      </select>
                    </label>
                  </>
                )}

                <div className="endpoint-heading">发件 SMTP</div>
                <label className="form-field">
                  <span>服务器</span>
                  <input {...register('outgoingHost')} autoComplete="off" placeholder="smtp.example.com" disabled={isVerifying} aria-invalid={Boolean(errors.outgoingHost)} />
                  {errors.outgoingHost && <em>{errors.outgoingHost.message}</em>}
                </label>
                <label className="form-field">
                  <span>端口</span>
                  <input {...register('outgoingPort')} inputMode="numeric" disabled={isVerifying} aria-invalid={Boolean(errors.outgoingPort)} />
                  {errors.outgoingPort && <em>{errors.outgoingPort.message}</em>}
                </label>
                <label className="form-field">
                  <span>用户名（可选）</span>
                  <input {...register('outgoingUsername')} autoComplete="off" placeholder="默认使用邮箱地址" disabled={isVerifying} />
                </label>
                <label className="form-field">
                  <span>TLS 模式</span>
                  <select {...register('outgoingTlsMode')} disabled={isVerifying}>
                    <option value="implicit">SMTPS（通常 465）</option>
                    <option value="starttls">STARTTLS（通常 587）</option>
                  </select>
                </label>
              </div>
            )}

            <div className="wizard-actions">
              <button className="text-action" type="button" disabled={isVerifying} onClick={returnToProviderSelection}>返回</button>
              <button className="primary-action" type="submit" disabled={isVerifying}>
                {isVerifying ? '正在验证…' : '验证并添加'}
                {!isVerifying && <Icon name="check" size={17} />}
              </button>
            </div>
          </form>
        )}
      </section>
    </div>
  );
}
