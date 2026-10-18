import { NextResponse } from "next/server";

const practiceAPI = process.env.PRACTICE_API_URL ?? "http://127.0.0.1:8787";

export async function proxyPractice(path: string, body = "{}"): Promise<NextResponse> {
  try {
    const response = await fetch(`${practiceAPI}${path}`, {
      method: "POST",
      body,
      cache: "no-store",
      headers: { "content-type": "application/json" },
    });
    const payload = await response.text();
    return new NextResponse(payload, {
      status: response.status,
      headers: {
        "cache-control": "no-store",
        "content-type": response.headers.get("content-type") ?? "application/json",
      },
    });
  } catch {
    return NextResponse.json(
      { code: "PRACTICE_SERVER_UNAVAILABLE", error: "The Go practice engine is not running." },
      { status: 503 },
    );
  }
}
