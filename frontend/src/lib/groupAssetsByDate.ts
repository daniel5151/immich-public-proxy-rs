import type { SafeAsset } from '../types/generated/SafeAsset';
import type { DateGroup } from '../types';

/**
 * Group a list of assets into runs of consecutive items that share the same
 * human-readable date label (e.g. "Mon, Jun 9, 2026"). The input order is
 * preserved, and each item keeps its global index into the source list so the
 * grid, lightbox, and scrubber can all key off one stable index space.
 */
export function groupAssetsByDate(assets: SafeAsset[]): DateGroup[] {
  const groups: DateGroup[] = [];
  assets.forEach((asset, i) => {
    let dateLabel = 'Unknown Date';
    if (asset.fileCreatedAt) {
      try {
        const date = new Date(asset.fileCreatedAt);
        if (!isNaN(date.getTime())) {
          dateLabel = date.toLocaleDateString('en-US', {
            weekday: 'short',
            month: 'short',
            day: 'numeric',
            year: 'numeric',
          });
        }
      } catch {
        // Fallback to the default label.
      }
    }

    if (groups.length > 0 && groups[groups.length - 1].label === dateLabel) {
      groups[groups.length - 1].items.push({ globalIndex: i, asset });
    } else {
      groups.push({
        label: dateLabel,
        items: [{ globalIndex: i, asset }],
      });
    }
  });
  return groups;
}
