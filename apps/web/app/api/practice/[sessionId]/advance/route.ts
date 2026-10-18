import { proxyPractice } from "../../../../../src/server/practice/proxy";

type Context = { params: Promise<{ sessionId: string }> };

export async function POST(request: Request, context: Context) {
  const { sessionId } = await context.params;
  return proxyPractice(
    `/v1/practice/${encodeURIComponent(sessionId)}/advance`,
    await request.text(),
  );
}
