import { useState } from 'react';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { zodResolver } from '@hookform/resolvers/zod';
import { Icon } from '../lib/icons';
import { appErrorMessage, createAccount, detectProvider, providerPresets, startSync } from '../lib/tauri';
import type { ProviderPreset } from '../types';

const accountSchema = z.object({
  email: z.string().email('请输入有效的邮箱地址'),
  displayName: z.string().min(1, '请输入显示名称'),
  secret: z.string().min(1, '请输入客户端授权码或安全凭据'),
  incomingHost: z.string().optional(),
  incomingPort: z.string().optional(),
  incomingUsername: z.string().optional(),
  incomingSecret: z.string().optional(),
  outgoingHost: z.string().optional(),
  outgoingPort: z.string().optional(),
  outgoingUsername: z.string().optional(),
  outgoingSecret: z.string().optional(),
});
type AccountForm = z.infer<typeof accountSchema>;

export function AccountWizard({ onClose, onSaved }: { onClose: () => void; onSaved: () => void }) {
  const [step, setStep] = useState(1);
  const [provider, setProvider] = useState<ProviderPreset | null>(null);
  const [status, setStatus] = useState('');
  const { register, handleSubmit, getValues, setError, formState: { errors } } = useForm<AccountForm>({ resolver: zodResolver(accountSchema), defaultValues: { email: '', displayName: '', secret: '', incomingHost: '', incomingPort: '993', incomingUsername: '', incomingSecret: '', outgoingHost: '', outgoingPort: '465', outgoingUsername: '', outgoingSecret: '' } });

  const identify = async () => {
    const email = getValues('email').trim();
    if (!z.string().email().safeParse(email).success) {
      setError('email', { type: 'manual', message: '请输入有效的邮箱地址' });
      return;
    }
    const result = await detectProvider(email);
    setProvider(result);
    setStep(2);
  };

  const save = async (values: AccountForm) => {
    setStatus('正在保存账户与安全凭据引用…');
    try {
      const providerId = provider?.id ?? 'generic';
      const isGeneric = providerId === 'generic';
      if (isGeneric && (!values.incomingHost?.trim() || !values.outgoingHost?.trim())) {
        setStatus('通用服务需要同时填写收件和发件服务器。');
        return;
      }
      const account = await createAccount({
        email: values.email,
        displayName: values.displayName,
        providerId,
        secret: values.secret,
        incomingSecret: values.incomingSecret?.trim() || undefined,
        outgoingSecret: values.outgoingSecret?.trim() || undefined,
        incoming: isGeneric ? { protocol: 'imap', host: values.incomingHost!.trim(), port: Number(values.incomingPort) || 993, tlsMode: 'implicit', authMethod: 'password', username: values.incomingUsername?.trim() || values.email } : undefined,
        outgoing: isGeneric ? { protocol: 'smtp', host: values.outgoingHost!.trim(), port: Number(values.outgoingPort) || 465, tlsMode: 'implicit', authMethod: 'password', username: values.outgoingUsername?.trim() || values.email } : undefined,
      });
      void startSync(account.id).catch(() => undefined);
      setStatus('账户已保存，后台将尝试建立收件连接。');
      window.setTimeout(onSaved, 500);
    } catch (error) {
      setStatus(appErrorMessage(error));
    }
  };

  return (
    <div className="modal-scrim" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <section className="wizard-dialog" role="dialog" aria-modal="true" aria-labelledby="wizard-title">
        <header className="wizard-header"><div><span className="compose-kicker">账户设置</span><h2 id="wizard-title">添加邮箱</h2></div><button className="icon-button" onClick={onClose} aria-label="关闭"><Icon name="close" size={20} /></button></header>
        <div className="stepper"><span className="step is-done">1</span><i /><span className={step >= 2 ? 'step is-current' : 'step'}>2</span><i /><span className={step >= 3 ? 'step is-current' : 'step'}>3</span></div>
        {step === 1 && <div className="wizard-step"><h3>先输入邮箱地址</h3><p className="helper-text">我们只会根据域名识别服务器，不会把密码发送到网络。</p><label className="form-field"><span>邮箱地址</span><input {...register('email')} type="email" placeholder="you@example.com" autoFocus />{errors.email && <em>{errors.email.message}</em>}</label><button className="primary-action full" type="button" onClick={() => void identify()}>识别邮箱服务 <Icon name="chevron" size={17} /></button><button className="text-action wizard-secondary-action" type="button" onClick={() => { setProvider(providerPresets.find((item) => item.id === 'cloudflare-smtp') ?? null); setStep(2); }}>只配置 Cloudflare 发件</button></div>}
        {step === 2 && <div className="wizard-step"><div className="provider-detected"><div className="provider-logo">{provider?.displayName.slice(0, 1) ?? '?'}</div><div><strong>{provider?.displayName ?? '通用邮件服务'}</strong><span>{provider ? '已匹配推荐配置' : '需要手动填写服务器'}</span></div><Icon name="checkCircle" size={22} /></div><div className="help-callout"><Icon name="shield" size={18} /><p>{provider?.helpText ?? '请准备收件和发件服务器的 TLS 配置。'}</p></div><label className="form-field"><span>显示名称</span><input {...register('displayName')} placeholder="例如：小林" />{errors.displayName && <em>{errors.displayName.message}</em>}</label><label className="form-field"><span>{provider?.id === 'qq' || provider?.id === 'netease-163' ? '客户端授权码' : '密码或令牌'}</span><input {...register('secret')} type="password" placeholder="只保存到系统安全存储" />{errors.secret && <em>{errors.secret.message}</em>}</label>{(provider?.id === 'generic' || !provider) && <div className="endpoint-grid"><div className="endpoint-heading">收件 IMAP</div><label className="form-field"><span>服务器</span><input {...register('incomingHost')} placeholder="imap.example.com" /></label><label className="form-field"><span>端口</span><input {...register('incomingPort')} inputMode="numeric" /></label><label className="form-field"><span>用户名</span><input {...register('incomingUsername')} placeholder="通常是完整邮箱地址" /></label><label className="form-field"><span>独立凭据（可选）</span><input {...register('incomingSecret')} type="password" placeholder="留空则使用上方凭据" /></label><div className="endpoint-heading">发件 SMTP</div><label className="form-field"><span>服务器</span><input {...register('outgoingHost')} placeholder="smtp.example.com" /></label><label className="form-field"><span>端口</span><input {...register('outgoingPort')} inputMode="numeric" /></label><label className="form-field"><span>用户名</span><input {...register('outgoingUsername')} placeholder="可与收件用户名不同" /></label><label className="form-field"><span>独立凭据（可选）</span><input {...register('outgoingSecret')} type="password" placeholder="留空则使用上方凭据" /></label></div>}<div className="wizard-actions"><button className="text-action" type="button" onClick={() => setStep(1)}>返回</button><button className="primary-action" type="button" onClick={() => setStep(3)}>继续测试 <Icon name="chevron" size={17} /></button></div></div>}
        {step === 3 && <form className="wizard-step" onSubmit={handleSubmit(save)}><h3>分别测试收件与发件</h3><p className="helper-text">两个连接相互独立。保存后桌面端会执行真实 TLS 探测并报告能力。</p>{(provider?.incoming || !provider || provider?.id === 'generic') ? <div className="test-row"><span className="test-icon"><Icon name="inbox" size={18} /></span><div><strong>收件连接 · IMAP</strong><span>{provider?.incoming ? `${provider.incoming.host}:${provider.incoming.port}` : `${getValues('incomingHost') || '手动配置'}:${getValues('incomingPort') || '993'}`}</span></div><button type="button" className="test-button" onClick={() => setStatus('收件端点格式已检查；保存账户后执行 TLS + CAPABILITY。')}>检查配置</button></div> : <div className="test-row is-disabled"><span className="test-icon"><Icon name="inbox" size={18} /></span><div><strong>收件连接 · 未配置</strong><span>这是仅发件账户，不会尝试连接收件服务器。</span></div></div>}<div className="test-row"><span className="test-icon"><Icon name="send" size={18} /></span><div><strong>发件连接 · SMTP</strong><span>{provider?.outgoing ? `${provider.outgoing.host}:${provider.outgoing.port}${provider.outgoing.username ? ` · ${provider.outgoing.username}` : ''}` : `${getValues('outgoingHost') || '手动配置'}:${getValues('outgoingPort') || '465'}`}</span></div><button type="button" className="test-button" onClick={() => setStatus('发件端点格式已检查；保存账户后执行 TLS 握手。')}>检查配置</button></div><div className="sync-range"><span>首次同步范围</span><button type="button" className="range-option is-selected">最近 3 个月</button><button type="button" className="range-option">全部邮件</button></div><div className="wizard-actions"><button className="text-action" type="button" onClick={() => setStep(2)}>返回</button><button className="primary-action" type="submit">保存并开始同步 <Icon name="check" size={17} /></button></div>{status && <div className="wizard-status"><Icon name="checkCircle" size={17} />{status}</div>}</form>}
      </section>
    </div>
  );
}
