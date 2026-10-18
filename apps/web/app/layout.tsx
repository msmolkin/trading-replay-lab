import type { Metadata } from "next";
import type { ReactNode } from "react";

import "../src/styles/globals.css";

export const metadata: Metadata = {
  title: "Trading Replay Lab · Can your strategy survive the path?",
  description:
    "Practice leveraged trading against a sealed deterministic market path with reversals, stops, and maker/taker constraints.",
  applicationName: "Trading Replay Lab",
  manifest: "/site.webmanifest",
  icons: {
    icon: [{ url: "/favicon.svg", type: "image/svg+xml" }],
    apple: [{ url: "/apple-touch-icon.png", sizes: "180x180", type: "image/png" }],
  },
  openGraph: {
    title: "Trading Replay Lab",
    description: "Can your trading survive an unrevealed market path?",
    type: "website",
  },
  twitter: {
    card: "summary",
    title: "Trading Replay Lab",
    description: "Can your trading survive an unrevealed market path?",
  },
};

export default function RootLayout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <head>
        <meta name="theme-color" content="#07101f" />
      </head>
      <body>{children}</body>
    </html>
  );
}
