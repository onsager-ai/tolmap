// Streamed fetch + off-main-thread parse for the two big documents this
// app loads whole: the map document and a district's symbols sibling
// (docs/API.md). Before this, both were one `fetch().then(r => r.json())`
// with no indicator -- fine for the small acceptance fixtures, not for
// dify's 6.7 MB map. This reads the body via a stream (so a caller can show
// bytes received, and the total when Content-Length is known) and parses
// off the main thread in a Web Worker (so a large document doesn't freeze
// panning/typing while it parses) -- see src/workers/jsonParse.worker.ts.
//
// TanStack Query caching semantics are unaffected: this is just what a
// queryFn awaits before returning the parsed value; nothing here bypasses
// or duplicates the query cache.
import { clearLoadProgress, setLoadProgress } from "./loadProgress";

export interface StreamProgress {
  receivedBytes: number;
  totalBytes: number | null;
}

async function readBodyText(response: Response, onProgress?: (progress: StreamProgress) => void): Promise<string> {
  const encoding = response.headers.get("content-encoding");
  const lengthHeader = response.headers.get("content-length");
  // A compressed body's Content-Length is the wire size, not the decoded
  // size this function returns -- see loadProgress.ts's own comment.
  const totalBytes = encoding && encoding !== "identity" ? null : lengthHeader ? Number(lengthHeader) : null;
  const reader = response.body?.getReader();
  if (!reader) {
    // No streaming body support (older Safari) -- fall back to the whole
    // thing at once, still reporting one progress frame so a caller's
    // indicator has something to show rather than nothing at all.
    const text = await response.text();
    onProgress?.({ receivedBytes: text.length, totalBytes });
    return text;
  }
  const decoder = new TextDecoder();
  let received = 0;
  let text = "";
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    received += value.byteLength;
    text += decoder.decode(value, { stream: true });
    onProgress?.({ receivedBytes: received, totalBytes });
  }
  text += decoder.decode();
  return text;
}

// One persistent worker, multiplexed by request id -- not one worker per
// parse call. District-symbol fetches happen several at a time
// (useDistrictSymbolsMap fires one query per wanted district), so a naive
// "attach a message listener, remove it on the first reply" pattern would
// let one call's listener consume another call's response.
let worker: Worker | undefined;
let nextRequestId = 0;
const pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();

interface ParseSuccess {
  id: number;
  ok: true;
  data: unknown;
}
interface ParseFailure {
  id: number;
  ok: false;
  error: string;
}

function getWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("../workers/jsonParse.worker.ts", import.meta.url), { type: "module" });
  worker.addEventListener("message", (event: MessageEvent<ParseSuccess | ParseFailure>) => {
    const entry = pending.get(event.data.id);
    if (!entry) return;
    pending.delete(event.data.id);
    if (event.data.ok) entry.resolve(event.data.data);
    else entry.reject(new Error(event.data.error));
  });
  return worker;
}

function parseJsonInWorker<T>(text: string): Promise<T> {
  const id = nextRequestId++;
  return new Promise<T>((resolve, reject) => {
    pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
    getWorker().postMessage({ id, text });
  });
}

/** Fetches `url`, reporting download (and then parse) progress under
 * `progressKey` (src/api/loadProgress.ts), and parses the body in the
 * worker above. `onResponse` is called once headers arrive, before the
 * body is read -- callers that need to turn a non-2xx response into their
 * own error type (ApiRequestError, in src/api/client.ts) do that there
 * rather than this module needing to know about that shape. */
export async function fetchJsonTracked<T>(
  url: string,
  progressKey: string,
  onResponse?: (response: Response) => void | Promise<void>,
): Promise<T> {
  try {
    const response = await fetch(url);
    if (onResponse) await onResponse(response);
    setLoadProgress(progressKey, { phase: "downloading", receivedBytes: 0, totalBytes: null });
    const text = await readBodyText(response, (progress) =>
      setLoadProgress(progressKey, { phase: "downloading", ...progress }),
    );
    setLoadProgress(progressKey, { phase: "parsing", receivedBytes: text.length, totalBytes: text.length });
    const data = await parseJsonInWorker<T>(text);
    return data;
  } finally {
    clearLoadProgress(progressKey);
  }
}
