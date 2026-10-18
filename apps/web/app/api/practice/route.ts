import { proxyPractice } from "../../../src/server/practice/proxy";

export const dynamic = "force-dynamic";

export function POST() {
  return proxyPractice("/v1/practice");
}
