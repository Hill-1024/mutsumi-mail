import type { ReactNode, SVGProps } from 'react';

export type IconName =
  | 'inbox'
  | 'star'
  | 'draft'
  | 'send'
  | 'archive'
  | 'trash'
  | 'search'
  | 'settings'
  | 'more'
  | 'refresh'
  | 'pen'
  | 'chevron'
  | 'back'
  | 'reply'
  | 'replyAll'
  | 'forward'
  | 'download'
  | 'paperclip'
  | 'check'
  | 'shield'
  | 'sun'
  | 'moon'
  | 'monitor'
  | 'close'
  | 'menu'
  | 'external'
  | 'clock'
  | 'folder'
  | 'plus'
  | 'checkCircle'
  | 'sendClock';

const paths: Record<IconName, ReactNode> = {
  inbox: <><path d="M4 5.5h16v13H4z" /><path d="M4 14h4l1.5 2h5L16 14h4" /></>,
  star: <path d="m12 3 2.7 5.47 6.03.88-4.36 4.25 1.03 6-5.4-2.84L6.6 19.6l1.03-6-4.36-4.25 6.03-.88L12 3Z" />,
  draft: <><path d="M4 5.5h16v13H4z" /><path d="m4.5 7 7.5 6 7.5-6" /></>,
  send: <><path d="m21 3-7.5 18-3.2-7.3L3 10.5 21 3Z" /><path d="m10.4 13.7 4.2-4.2" /></>,
  archive: <><path d="M4 7h16v13H4z" /><path d="M3 4h18v3H3zM9 11h6" /></>,
  trash: <><path d="M5.5 7.5h13l-1 13h-10z" /><path d="M4 5h16M9 5V3h6v2M10 10v7M14 10v7" /></>,
  search: <><circle cx="10.8" cy="10.8" r="6.2" /><path d="m16 16 4.3 4.3" /></>,
  settings: <><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-1.9 1.9-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.03 1.56V20h-2.68v-.09a1.7 1.7 0 0 0-1.03-1.56 1.7 1.7 0 0 0-1.88.34l-.06.06-1.9-1.9.06-.06A1.7 1.7 0 0 0 7.8 15a1.7 1.7 0 0 0-1.56-1.03H6v-2.68h.24A1.7 1.7 0 0 0 7.8 10.26a1.7 1.7 0 0 0-.34-1.88L7.4 8.3l1.9-1.9.06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1.03-1.56V5H15v.24a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 1.9 1.9-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.56 1.03H21v2.68h-.24A1.7 1.7 0 0 0 19.4 15Z" /></>,
  more: <><circle cx="5" cy="12" r="1" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" /><circle cx="19" cy="12" r="1" fill="currentColor" stroke="none" /></>,
  refresh: <><path d="M20 11a8 8 0 0 0-14.9-3L3 11" /><path d="M3 5v6h6M4 13a8 8 0 0 0 14.9 3L21 13" /><path d="M21 19v-6h-6" /></>,
  pen: <><path d="m4 20 4.2-1 9.9-9.9a2.1 2.1 0 0 0-3-3L5.2 16 4 20Z" /><path d="m13.8 7.2 3 3" /></>,
  chevron: <path d="m9 6 6 6-6 6" />,
  back: <path d="m15 5-7 7 7 7" />,
  reply: <><path d="M9 8 4 12l5 4" /><path d="M5 12h8a6 6 0 0 1 6 6" /></>,
  replyAll: <><path d="m8 8-5 4 5 4" /><path d="m12 8-5 4 5 4" /><path d="M8 12h5a6 6 0 0 1 6 6" /></>,
  forward: <><path d="m15 8 5 4-5 4" /><path d="M19 12h-8a6 6 0 0 0-6 6" /></>,
  download: <><path d="M12 3v12" /><path d="m7 10 5 5 5-5M4 20h16" /></>,
  paperclip: <path d="m20.5 11.5-8.9 8.9a5 5 0 0 1-7.1-7.1l9.2-9.2a3.4 3.4 0 1 1 4.8 4.8l-9.2 9.2a1.8 1.8 0 0 1-2.5-2.5l8.4-8.4" />,
  check: <path d="m5 12 4.5 4.5L19 7" />,
  shield: <><path d="M12 3 20 6v5c0 5-3.4 8.5-8 10-4.6-1.5-8-5-8-10V6l8-3Z" /><path d="m8.5 12 2.2 2.2 4.8-5" /></>,
  sun: <><circle cx="12" cy="12" r="3.5" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></>,
  moon: <path d="M20 15.5A8.5 8.5 0 0 1 8.5 4 8.5 8.5 0 1 0 20 15.5Z" />,
  monitor: <><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></>,
  close: <><path d="m6 6 12 12M18 6 6 18" /></>,
  menu: <><path d="M4 7h16M4 12h16M4 17h16" /></>,
  external: <><path d="M14 4h6v6M20 4l-9 9" /><path d="M18 13v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h5" /></>,
  clock: <><circle cx="12" cy="12" r="8.5" /><path d="M12 7v5l3.5 2" /></>,
  folder: <path d="M3.5 6.5h6l2 2h9v9.5h-17z" />,
  plus: <><path d="M12 5v14M5 12h14" /></>,
  checkCircle: <><circle cx="12" cy="12" r="9" /><path d="m8 12 2.5 2.5L16 9" /></>,
  sendClock: <><path d="m20 4-7.5 18-3.2-7.3L2 11.5 20 4Z" /><path d="M16 17v3M16 17a3 3 0 1 0 3 3" /></>,
};

export function Icon({ name, size = 20, strokeWidth = 1.8, ...props }: SVGProps<SVGSVGElement> & { name: IconName; size?: number; strokeWidth?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>
      {paths[name]}
    </svg>
  );
}
