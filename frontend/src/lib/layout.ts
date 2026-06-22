/**
 * Shared layout offsets for anchoring content under the sticky header.
 *
 * HEADER_GAP: breathing room left below the header when an element is scrolled
 *   to the top.
 * ACTIVE_LINE_GAP: extra drop to the notional "active line" the date tracker
 *   uses to decide which group is current.
 *
 * Kept in one module because both the page-level scroll helpers and the date
 * scrubber's indicator math key off the same numbers; duplicating them risks
 * the indicator and the scroll target drifting apart.
 */
export const HEADER_GAP = 10;
export const ACTIVE_LINE_GAP = 20;
