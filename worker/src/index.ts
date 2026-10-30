// POST /mydiskisfull/v1/folder {"name", "bytes", "macos"}: what's known of a
// folder, from what was said of it before or from Claude. Only the name (as
// the app's rule makes it), a rounded size and a macOS version come in;
// nothing about who asked is kept or logged.

import Anthropic from "@anthropic-ai/sdk";
import { ask, MODEL, type Answer, type Question } from "./ask";
import { askable } from "./name";

const PATH = "/mydiskisfull/v1/folder";
/** Answers Claude couldn't give are asked again after this long. */
const RETRY_UNKNOWN = 30 * 86_400;

interface Said extends Answer {
  name: string;
  by: string;
  cached: boolean;
}

function reply(status: number, body: object): Response {
  return Response.json(body, { status, headers: { "cache-control": "no-store" } });
}

function question(body: unknown): Question | null {
  if (typeof body !== "object" || body === null) return null;
  const { name, bytes, macos } = body as Record<string, unknown>;
  if (typeof name !== "string" || !askable(name)) return null;
  if (typeof bytes !== "number" || !Number.isSafeInteger(bytes) || bytes < 0) return null;
  // Only a version: it goes into the question, and the answer is everyone's.
  if (typeof macos !== "string" || !/^\d{2}(\.\d{1,2}){0,2}$/.test(macos)) return null;
  return { name, bytes, macos };
}

/** A hash of the address, so the rate limiter never holds the address itself. */
async function asker(request: Request): Promise<string> {
  const ip = request.headers.get("cf-connecting-ip") ?? "";
  const hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`mydiskisfull:${ip}`));
  return btoa(String.fromCharCode(...new Uint8Array(hash)));
}

export default {
  async fetch(request, env, ctx): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname !== PATH) return reply(404, { error: "not_found" });
    if (request.method !== "POST") return reply(405, { error: "post" });
    if (Number(request.headers.get("content-length") ?? "0") > 2048) {
      return reply(413, { error: "too_big" });
    }
    let q: Question | null;
    try {
      q = question(await request.json());
    } catch {
      q = null;
    }
    if (!q) return reply(400, { error: "name" });
    const key = q.name.toLowerCase();
    const now = Math.floor(Date.now() / 1000);

    const kept = await env.ANSWERS.prepare(
      "SELECT name, known, what, grows, helps, model, created FROM answers WHERE key = ?",
    )
      .bind(key)
      .first<{ name: string; known: number; what: string | null; grows: string | null; helps: string | null; model: string; created: number }>();
    if (kept && (kept.known || now - kept.created < RETRY_UNKNOWN)) {
      ctx.waitUntil(
        env.ANSWERS.prepare("UPDATE answers SET asked = asked + 1 WHERE key = ?").bind(key).run(),
      );
      const said: Said = {
        name: kept.name,
        known: kept.known === 1,
        what: kept.what,
        grows: kept.grows,
        helps: kept.helps,
        by: kept.model,
        cached: true,
      };
      return reply(200, said);
    }

    if (!(await env.ASKS.limit({ key: await asker(request) })).success) {
      return reply(429, { error: "busy" });
    }
    const day = new Date(now * 1000).toISOString().slice(0, 10);
    const counted = await env.ANSWERS.prepare(
      "INSERT INTO days (day, asks) VALUES (?, 1) ON CONFLICT (day) DO UPDATE SET asks = asks + 1 RETURNING asks",
    )
      .bind(day)
      .first<{ asks: number }>();
    if ((counted?.asks ?? Infinity) > Number(env.DAILY_ASKS)) {
      return reply(503, { error: "today" });
    }

    let answer: Answer;
    try {
      const client = new Anthropic({
        apiKey: env.ANTHROPIC_API_KEY,
        baseURL: env.ANTHROPIC_BASE_URL || undefined,
        maxRetries: 1,
        timeout: 60_000,
      });
      answer = await ask(client, q);
    } catch (e) {
      const status = e instanceof Anthropic.APIError ? e.status : undefined;
      console.error(JSON.stringify({ message: "ask failed", status, error: e instanceof Error ? e.message : String(e) }));
      return reply(502, { error: "ai" });
    }
    await env.ANSWERS.prepare(
      `INSERT INTO answers (key, name, known, what, grows, helps, model, macos, created)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT (key) DO UPDATE SET known = excluded.known, what = excluded.what,
         grows = excluded.grows, helps = excluded.helps, model = excluded.model,
         macos = excluded.macos, created = excluded.created, asked = asked + 1`,
    )
      .bind(key, q.name, answer.known ? 1 : 0, answer.what, answer.grows, answer.helps, MODEL, q.macos, now)
      .run();
    console.log(JSON.stringify({ message: "asked", known: answer.known }));
    const said: Said = { name: q.name, ...answer, by: MODEL, cached: false };
    return reply(200, said);
  },
} satisfies ExportedHandler<Env>;
