import { Icon as IconifyIcon, type IconProps as IconifyIconProps } from '@iconify/react/offline';
import addRounded from '@iconify-icons/material-symbols/add-rounded';
import archiveRounded from '@iconify-icons/material-symbols/archive-rounded';
import archiveOutlineRounded from '@iconify-icons/material-symbols/archive-outline-rounded';
import arrowBackRounded from '@iconify-icons/material-symbols/arrow-back-rounded';
import attachFileRounded from '@iconify-icons/material-symbols/attach-file-rounded';
import checkCircleOutlineRounded from '@iconify-icons/material-symbols/check-circle-outline-rounded';
import checkRounded from '@iconify-icons/material-symbols/check-rounded';
import chevronRightRounded from '@iconify-icons/material-symbols/chevron-right-rounded';
import closeRounded from '@iconify-icons/material-symbols/close-rounded';
import darkModeOutlineRounded from '@iconify-icons/material-symbols/dark-mode-outline-rounded';
import deleteOutlineRounded from '@iconify-icons/material-symbols/delete-outline-rounded';
import deleteRounded from '@iconify-icons/material-symbols/delete-rounded';
import desktopWindowsOutlineRounded from '@iconify-icons/material-symbols/desktop-windows-outline-rounded';
import draftOutlineRounded from '@iconify-icons/material-symbols/draft-outline-rounded';
import draftRounded from '@iconify-icons/material-symbols/draft-rounded';
import editOutlineRounded from '@iconify-icons/material-symbols/edit-outline-rounded';
import folderOutlineRounded from '@iconify-icons/material-symbols/folder-outline-rounded';
import folderRounded from '@iconify-icons/material-symbols/folder-rounded';
import forwardRounded from '@iconify-icons/material-symbols/forward-rounded';
import inboxOutlineRounded from '@iconify-icons/material-symbols/inbox-outline-rounded';
import inboxRounded from '@iconify-icons/material-symbols/inbox-rounded';
import lightModeOutlineRounded from '@iconify-icons/material-symbols/light-mode-outline-rounded';
import menuRounded from '@iconify-icons/material-symbols/menu-rounded';
import moreHorizRounded from '@iconify-icons/material-symbols/more-horiz-rounded';
import openInNewRounded from '@iconify-icons/material-symbols/open-in-new-rounded';
import outboxRounded from '@iconify-icons/material-symbols/outbox-rounded';
import outboxOutlineRounded from '@iconify-icons/material-symbols/outbox-outline-rounded';
import refreshRounded from '@iconify-icons/material-symbols/refresh-rounded';
import replyAllRounded from '@iconify-icons/material-symbols/reply-all-rounded';
import replyRounded from '@iconify-icons/material-symbols/reply-rounded';
import scheduleRounded from '@iconify-icons/material-symbols/schedule-rounded';
import searchRounded from '@iconify-icons/material-symbols/search-rounded';
import sendOutlineRounded from '@iconify-icons/material-symbols/send-outline-rounded';
import sendRounded from '@iconify-icons/material-symbols/send-rounded';
import settingsOutlineRounded from '@iconify-icons/material-symbols/settings-outline-rounded';
import settingsRounded from '@iconify-icons/material-symbols/settings-rounded';
import shieldOutlineRounded from '@iconify-icons/material-symbols/shield-outline-rounded';
import starOutlineRounded from '@iconify-icons/material-symbols/star-outline-rounded';
import starRounded from '@iconify-icons/material-symbols/star-rounded';

export type IconName =
  | 'inbox'
  | 'inboxFilled'
  | 'star'
  | 'starFilled'
  | 'draft'
  | 'draftFilled'
  | 'send'
  | 'sendFilled'
  | 'archive'
  | 'archiveFilled'
  | 'trash'
  | 'trashFilled'
  | 'search'
  | 'settings'
  | 'settingsFilled'
  | 'more'
  | 'refresh'
  | 'pen'
  | 'chevron'
  | 'back'
  | 'reply'
  | 'replyAll'
  | 'forward'
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
  | 'folderFilled'
  | 'plus'
  | 'checkCircle'
  | 'sendClock'
  | 'sendClockFilled';

const icons = {
  inbox: inboxOutlineRounded,
  inboxFilled: inboxRounded,
  star: starOutlineRounded,
  starFilled: starRounded,
  draft: draftOutlineRounded,
  draftFilled: draftRounded,
  send: sendOutlineRounded,
  sendFilled: sendRounded,
  archive: archiveOutlineRounded,
  archiveFilled: archiveRounded,
  trash: deleteOutlineRounded,
  trashFilled: deleteRounded,
  search: searchRounded,
  settings: settingsOutlineRounded,
  settingsFilled: settingsRounded,
  more: moreHorizRounded,
  refresh: refreshRounded,
  pen: editOutlineRounded,
  chevron: chevronRightRounded,
  back: arrowBackRounded,
  reply: replyRounded,
  replyAll: replyAllRounded,
  forward: forwardRounded,
  paperclip: attachFileRounded,
  check: checkRounded,
  shield: shieldOutlineRounded,
  sun: lightModeOutlineRounded,
  moon: darkModeOutlineRounded,
  monitor: desktopWindowsOutlineRounded,
  close: closeRounded,
  menu: menuRounded,
  external: openInNewRounded,
  clock: scheduleRounded,
  folder: folderOutlineRounded,
  folderFilled: folderRounded,
  plus: addRounded,
  checkCircle: checkCircleOutlineRounded,
  sendClock: outboxOutlineRounded,
  sendClockFilled: outboxRounded,
} satisfies Record<IconName, IconifyIconProps['icon']>;

type AppIconProps = Omit<IconifyIconProps, 'icon' | 'height' | 'width'> & {
  name: IconName;
  size?: number;
  strokeWidth?: number;
  title?: string;
};

export function Icon({ name, size = 20, strokeWidth: _strokeWidth, title, ...props }: AppIconProps) {
  return (
    <IconifyIcon
      {...props}
      icon={icons[name]}
      width={size}
      height={size}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    />
  );
}
