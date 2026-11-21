import type { AttachmentOrigin } from '@hangar/core';
import * as m from '../../paraglide/messages';

export const originLabel = (o: AttachmentOrigin) => (o === 'agent' ? m.media_origin_agent() : m.media_origin_user());
