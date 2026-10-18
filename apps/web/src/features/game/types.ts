export type OrderIntent = "BUY" | "SELL" | "SHORT" | "COVER" | "REVERSE" | "CLOSE";
export type PriceMode = "MARKET" | "BID" | "ASK" | "MID" | "STOP";
export type OrderStatus = "FILLED" | "WORKING" | "CANCELED" | "REJECTED";

export type PracticeOrder = {
  id: number;
  intent: OrderIntent;
  side: "BUY" | "SELL";
  quantityMilli: number;
  filledMilli: number;
  remainingMilli: number;
  priceCents: number | null;
  stopPriceCents: number | null;
  reduceOnly: boolean;
  postOnly: boolean;
  marketableOnly: boolean;
  status: OrderStatus;
  reasonCode: string;
  message: string;
  createdAtStep: number;
};

export type RevealedPrice = {
  step: number;
  priceCents: number;
};

export type PracticeSnapshot = {
  sessionId: string;
  instrument: "SYNTH-BTC-USD";
  fidelity: "F1 SIMULATED";
  status: "ACTIVE" | "COMPLETED" | "LIQUIDATED";
  step: number;
  totalSteps: number;
  revealed: RevealedPrice[];
  markPriceCents: number;
  bidPriceCents: number;
  askPriceCents: number;
  positionMilli: number;
  averageEntryCents: number | null;
  realizedPnlCents: number;
  unrealizedPnlCents: number;
  feesCents: number;
  equityCents: number;
  startingEquityCents: number;
  leverage: number;
  effectiveLeverageMilli: number;
  initialMarginCents: number;
  maintenanceMarginCents: number;
  availableMarginCents: number;
  liquidationBufferCents: number;
  maxDrawdownCents: number;
  peakEquityCents: number;
  orders: PracticeOrder[];
};

export type SubmitPracticeOrder = {
  intent: OrderIntent;
  quantityMilli: number;
  priceMode: PriceMode;
  stopPriceCents?: number;
  reduceOnly: boolean;
  postOnly: boolean;
  marketableOnly: boolean;
};
