import type { AccountIcon, MachineIcon, TagColor } from '@hangar/core';
import type { IconName } from '../../ui/Icon';
import * as m from '../../paraglide/messages';

// Chaves do servidor → ícone e nome desta interface. `provider` não tem Lucide: é o ProviderGlyph.
export const MACHINE_ICON_NAME: Record<MachineIcon, IconName> = {
  laptop: 'Laptop', monitor: 'Monitor', server: 'Server', house: 'House', briefcase: 'Briefcase', cloud: 'Cloud',
};

export const ACCOUNT_ICON_NAME: Record<Exclude<AccountIcon, 'provider'>, IconName> = {
  'credit-card': 'CreditCard', 'key-round': 'KeyRound', wallet: 'Wallet', 'user-round': 'UserRound',
  'circle-user': 'CircleUser', contact: 'Contact', 'badge-check': 'BadgeCheck', 'at-sign': 'AtSign',
  ticket: 'Ticket', 'id-card': 'IdCard',
};

// Nomes lidos na hora do uso: quem importa só os ícones (a linha da lista) não toca nas mensagens.
export const MACHINE_ICON_LABEL: Record<MachineIcon, () => string> = {
  laptop: () => m.tags_icon_laptop(), monitor: () => m.tags_icon_monitor(), server: () => m.tags_icon_server(),
  house: () => m.tags_icon_house(), briefcase: () => m.tags_icon_briefcase(), cloud: () => m.tags_icon_cloud(),
};

export const ACCOUNT_ICON_LABEL: Record<AccountIcon, () => string> = {
  'credit-card': () => m.tags_icon_credit_card(), provider: () => m.tags_icon_provider(),
  'key-round': () => m.tags_icon_key_round(), wallet: () => m.tags_icon_wallet(),
  'user-round': () => m.tags_icon_user_round(), 'circle-user': () => m.tags_icon_circle_user(),
  contact: () => m.tags_icon_contact(), 'badge-check': () => m.tags_icon_badge_check(),
  'at-sign': () => m.tags_icon_at_sign(), ticket: () => m.tags_icon_ticket(), 'id-card': () => m.tags_icon_id_card(),
};

export const COLOR_LABEL: Record<TagColor, () => string> = {
  slate: () => m.tags_color_slate(), blue: () => m.tags_color_blue(), green: () => m.tags_color_green(),
  lime: () => m.tags_color_lime(), amber: () => m.tags_color_amber(), orange: () => m.tags_color_orange(),
  pink: () => m.tags_color_pink(), violet: () => m.tags_color_violet(),
};
