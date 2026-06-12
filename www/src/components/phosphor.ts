import article from '@phosphor-icons/core/duotone/article-duotone.svg?raw';
import clockCounterClockwise from '@phosphor-icons/core/duotone/clock-counter-clockwise-duotone.svg?raw';
import cloudCheck from '@phosphor-icons/core/duotone/cloud-check-duotone.svg?raw';
import database from '@phosphor-icons/core/duotone/database-duotone.svg?raw';
import gitMerge from '@phosphor-icons/core/duotone/git-merge-duotone.svg?raw';
import laptop from '@phosphor-icons/core/duotone/laptop-duotone.svg?raw';
import lightning from '@phosphor-icons/core/duotone/lightning-duotone.svg?raw';
import table from '@phosphor-icons/core/duotone/table-duotone.svg?raw';

/**
 * The Phosphor duotone icons used on the site, inlined at build time.
 * To use a new icon, import its `?raw` source above and register it here.
 */
export const phosphor = {
	article,
	'clock-counter-clockwise': clockCounterClockwise,
	'cloud-check': cloudCheck,
	database,
	'git-merge': gitMerge,
	laptop,
	lightning,
	table,
} as const;

export type PhosphorIconName = keyof typeof phosphor;
