export const resources = {
  'zh-CN': {
    appName: 'Mutsumi Mail',
    inbox: '收件箱',
    unifiedInbox: '统一收件箱',
    compose: '撰写邮件',
    offlineReady: '离线可用',
  },
  'en-US': {
    appName: 'Mutsumi Mail',
    inbox: 'Inbox',
    unifiedInbox: 'Unified inbox',
    compose: 'Compose',
    offlineReady: 'Offline ready',
  },
} as const;

export type Locale = keyof typeof resources;
