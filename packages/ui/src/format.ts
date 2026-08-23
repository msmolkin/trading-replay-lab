export function formatFixedPoint(atoms: bigint, scale: number): string {
  if (!Number.isInteger(scale) || scale < 0 || scale > 18) {
    throw new RangeError("scale must be an integer from zero through eighteen");
  }
  const negative = atoms < 0n;
  const magnitude = negative ? -atoms : atoms;
  if (scale === 0) {
    return `${negative ? "-" : ""}${magnitude}`;
  }
  const digits = magnitude.toString(10).padStart(scale + 1, "0");
  const whole = digits.slice(0, -scale);
  const fraction = digits.slice(-scale);
  return `${negative ? "-" : ""}${whole}.${fraction}`;
}

export function formatSignedPpb(ratePpb: bigint, fractionDigits = 2): string {
  if (!Number.isInteger(fractionDigits) || fractionDigits < 0 || fractionDigits > 9) {
    throw new RangeError("fractionDigits must be an integer from zero through nine");
  }
  const scaled = formatFixedPoint(ratePpb, 9);
  const [whole, fraction = ""] = scaled.split(".");
  if (fractionDigits === 0) {
    return whole;
  }
  return `${whole}.${fraction.slice(0, fractionDigits).padEnd(fractionDigits, "0")}`;
}

export type CalendarVisibility = "ABSOLUTE" | "RELATIVE" | "HIDDEN_UNTIL_COMPLETE";

export function formatSessionTime(
  visibility: CalendarVisibility,
  relativeNs: bigint,
  absoluteIso?: string,
): string {
  if (visibility === "ABSOLUTE") {
    if (!absoluteIso) {
      throw new Error("absoluteIso is required for ABSOLUTE calendar visibility");
    }
    return absoluteIso;
  }
  const seconds = relativeNs / 1_000_000_000n;
  const sign = seconds < 0n ? "-" : "+";
  const magnitude = seconds < 0n ? -seconds : seconds;
  const hours = magnitude / 3600n;
  const minutes = (magnitude % 3600n) / 60n;
  const remainingSeconds = magnitude % 60n;
  const relative = `${sign}${hours.toString().padStart(2, "0")}:${minutes
    .toString()
    .padStart(2, "0")}:${remainingSeconds.toString().padStart(2, "0")}`;
  return visibility === "RELATIVE" ? relative : `Sealed time ${relative}`;
}
