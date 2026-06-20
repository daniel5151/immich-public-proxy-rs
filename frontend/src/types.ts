import type { SafeAsset } from './types/generated/SafeAsset';

/** A run of consecutive assets that share the same human-readable date label. */
export interface DateGroup {
  label: string;
  items: { globalIndex: number; asset: SafeAsset }[];
}

/**
 * Controller shape shared by both upload-status strategies (batched poll and
 * SSE stream). `uploadFiles` is written against this interface so it doesn't
 * care which implementation it was handed.
 */
export interface UploadStatusController {
  /** Session token, present only for the SSE variant (tags each upload POST). */
  sessionToken?: string;
  /** Register a freshly-uploaded asset id (and, for SSE, open the stream). */
  add: (id: string) => void;
  /** Signal that no more ids are coming so the watcher can drain and exit. */
  done: () => void;
}
