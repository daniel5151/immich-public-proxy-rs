import type { SafeAsset } from '../types/generated/SafeAsset';
import type { UploadStatusController } from '../types';

/**
 * Callbacks the upload-status watchers use to report back into the gallery.
 * Keeping these as injected callbacks (rather than reaching into component
 * state) is what lets both factories live outside the component.
 */
export interface UploadStatusHandlers {
  /** A fully-processed asset is ready; insert it into the gallery. */
  onAssetReady: (asset: SafeAsset) => void;
}

/**
 * Batched client-poll status watcher.
 *
 * Polls the batched status endpoint for a whole set of just-uploaded assets
 * with a SINGLE loop, instead of one fetch-loop per asset. A large album drop
 * used to open N concurrent 500ms loops (~2N req/s, each re-validating the
 * share key upstream) which is exactly what trips edge rate-limiters like
 * CrowdSec. Here we keep one pending set, ask the server only about ids that
 * are still pending, and back off 500ms -> 1s -> 2s (capped). One request per
 * tick for the batch.
 *
 * Returns a controller so the upload loop can feed in asset ids the moment
 * each individual upload finishes — assets stream back into the gallery as
 * they're ready, rather than waiting for the whole batch to upload.
 */
export function startBatchStatusPoll(
  realKey: string,
  handlers: UploadStatusHandlers,
): UploadStatusController {
  const pending = new Set<string>();
  let uploadsDone = false;

  const MAX_IDS_PER_REQUEST = 256;
  const MAX_WALL_MS = 120_000; // overall ceiling, measured from first id added

  const loop = async () => {
    let delay = 500;

    // Wait for the first id before starting the wall-clock ceiling.
    while (pending.size === 0 && !uploadsDone) {
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    if (pending.size === 0) return; // nothing uploaded successfully
    const started = Date.now();

    while ((pending.size > 0 || !uploadsDone) && Date.now() - started < MAX_WALL_MS) {
      await new Promise((resolve) => setTimeout(resolve, delay));
      delay = Math.min(delay * 2, 2000);

      if (pending.size === 0) {
        // Drained, but more uploads may still be in flight — keep waiting cheaply.
        if (uploadsDone) break;
        delay = 500;
        continue;
      }

      // Chunk in the unlikely event the pending set exceeds the server-side id cap.
      const ids = Array.from(pending).slice(0, MAX_IDS_PER_REQUEST);
      try {
        const res = await fetch(
          `/share/${realKey}/status?ids=${encodeURIComponent(ids.join(','))}`,
        );
        if (res.status !== 200) {
          console.error(`Batch status polling failed: ${res.status}`);
          return;
        }
        const body: {
          ready: SafeAsset[];
          pending: string[];
          errored: string[];
        } = await res.json();

        for (const asset of body.ready) {
          handlers.onAssetReady(asset);
          pending.delete(asset.id);
        }
        for (const id of body.errored) {
          console.error(`Status polling reported error for asset ${id}`);
          pending.delete(id);
        }
        // A ready/errored asset resets the cadence so siblings stream in quickly.
        if (body.ready.length > 0 || body.errored.length > 0) delay = 500;
        // Anything the server didn't mention stays pending for the next tick.
      } catch (err) {
        console.error('Batch status polling request failed:', err);
        // Transient network error — keep the pending set and retry after backoff.
      }
    }

    if (pending.size > 0) {
      console.warn(`Batch status polling timed out for ${pending.size} asset(s)`);
    }
  };

  void loop();

  return {
    add: (id: string) => {
      pending.add(id);
    },
    done: () => {
      uploadsDone = true;
    },
  };
}

/**
 * Server-Sent Events alternative to {@link startBatchStatusPoll}. Instead of
 * the client re-requesting `/status?ids=...` on a backoff timer, we open ONE
 * long-lived EventSource and the server pushes each asset as it finishes. That
 * collapses an entire album drop to a single connection (zero per-tick
 * requests), which is the friendliest possible shape for the edge rate-limiter
 * (CrowdSec) that the whole batching effort exists to appease.
 *
 * SESSION-SCOPED design (the key improvement): the server owns the pending
 * set, keyed by a session token we mint here (`sessionToken`) and send on every
 * upload POST. The stream is opened for that SESSION, not a frozen id list, so
 * the server picks up assets uploaded AFTER the stream opened. That removes the
 * old "open at done()" compromise — we can open the stream the moment the FIRST
 * upload is dispatched, so photos stream back into the gallery while later files
 * are still uploading (real progressive appearance, best-possible
 * time-to-first-photo).
 *
 * Why not WebSocket (which would also give a dynamic set)? A WS upgrade
 * reintroduces the edge-layer fragility (1006 drops behind cloudflared/CrowdSec)
 * that plain-HTTP SSE survives, and we'd lose EventSource's free auto-reconnect.
 * Keeping SSE and moving the set server-side gets the dynamic behavior at no
 * transport risk.
 *
 * `add(id)` is now essentially bookkeeping — the server already learned the id
 * from the upload POST's `?session=` param. We use the first `add` only as the
 * trigger to open the stream. `done()` fires the finish beacon so the server can
 * end the stream as soon as the set drains (instead of waiting out its wall
 * clock).
 *
 * Reconnect note: the browser auto-reconnects an EventSource on transient drops;
 * we don't set event ids, so a reconnect just re-opens the same session URL and
 * the server re-resolves whatever is still pending (resolved assets were removed
 * from both PROCESSED_ASSETS and the session registry server-side, so they won't
 * reappear). We close on the terminal `done` event to stop the auto-reconnect
 * loop.
 */
export function startStatusStream(
  realKey: string,
  handlers: UploadStatusHandlers,
): UploadStatusController {
  // Client-minted session token, tying this drop's uploads to one stream. UUID is
  // alphanumeric + '-', which the server's is_safe_param validation already allows.
  const sessionToken =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID()
      : `sess-${Date.now()}-${Math.random().toString(36).slice(2)}`;

  let uploadsDone = false;
  let es: EventSource | null = null;

  const open = () => {
    if (es) return;
    es = new EventSource(
      `/share/${realKey}/status/stream?session=${encodeURIComponent(sessionToken)}`,
    );

    // `ready` — a fully processed asset; insert exactly like the poll path does.
    es.addEventListener('ready', (ev) => {
      try {
        const asset: SafeAsset = JSON.parse((ev as MessageEvent).data);
        handlers.onAssetReady(asset);
      } catch (err) {
        console.error('SSE: failed to parse ready asset:', err);
      }
    });

    // `errored` — the server gave up on this id; log and move on (matches poll).
    es.addEventListener('errored', (ev) => {
      console.error(`SSE reported error for asset ${(ev as MessageEvent).data}`);
    });

    // `done` — terminal: the server resolved everything (or hit its wall clock).
    // Close so the browser doesn't auto-reconnect to a finished stream.
    es.addEventListener('done', (ev) => {
      try {
        const payload: { resolved: number; pending: string[] } = JSON.parse(
          (ev as MessageEvent).data,
        );
        if (payload.pending && payload.pending.length > 0) {
          console.warn(`SSE stream ended with ${payload.pending.length} asset(s) still pending`);
        }
      } catch {
        // bare `done` fallback carries no JSON — nothing to read, just close.
      }
      es?.close();
      es = null;
    });

    // Transport-level error. EventSource will try to reconnect on its own; if the
    // uploads are already finished and we're not making progress, give up so we
    // don't hold a doomed connection open.
    es.onerror = () => {
      console.error('SSE stream connection error');
      if (uploadsDone) {
        es?.close();
        es = null;
      }
    };
  };

  return {
    // Expose the session token so uploadFiles can attach it to each upload POST.
    sessionToken,
    // The server already knows this id (via the upload POST's ?session= param); we
    // only use the first add() as the cue to open the stream, so assets start
    // streaming back while later files in the drop are still uploading.
    add: (_id: string) => {
      open();
    },
    done: () => {
      uploadsDone = true;
      // Fire-and-forget finish beacon so the server can end the stream as soon as
      // the pending set drains rather than waiting out its wall clock. keepalive
      // lets it survive if the tab is closing. A failure here is non-fatal — the
      // server's wall-clock guard still terminates the stream.
      void fetch(
        `/share/${realKey}/upload/finish?session=${encodeURIComponent(sessionToken)}`,
        { method: 'POST', keepalive: true },
      ).catch(() => {
        /* best-effort; wall clock is the backstop */
      });
      // If no uploads ever succeeded the stream was never opened — nothing to do.
    },
  };
}
