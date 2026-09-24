// A Vite worker module (loaded via `new Worker(new URL(...), { type:
// "module" })`, see src/api/streaming.ts) that does nothing but
// `JSON.parse` off the main thread. dify's map is 6.7 MB of JSON --
// parsing it inline blocks paint for long enough to feel like a hang, and
// this is the one thing to move off the main thread to fix that (the fetch
// itself was never the blocking part; streaming it only gets us a progress
// number, not the un-block -- V8's JSON.parse is synchronous regardless of
// how the bytes arrived).
//
// Typed against the "DOM" lib (this repo does not add "webworker" to
// tsconfig -- combining the two libs conflicts on the global `self`
// declaration), so `self` here is typed as `Window`. That's the wrong
// runtime object's *type*, but the calls this file makes -- addEventListener
// with a "message" listener, postMessage with a single argument -- both
// have overloads Window's own typings satisfy, so nothing here needs a cast
// or a second tsconfig project just to type-check.
interface ParseRequest {
  id: number;
  text: string;
}

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

self.addEventListener("message", (event: MessageEvent<ParseRequest>) => {
  const { id, text } = event.data;
  try {
    const data: unknown = JSON.parse(text);
    const response: ParseSuccess = { id, ok: true, data };
    self.postMessage(response);
  } catch (error) {
    const response: ParseFailure = { id, ok: false, error: error instanceof Error ? error.message : String(error) };
    self.postMessage(response);
  }
});

export {};
