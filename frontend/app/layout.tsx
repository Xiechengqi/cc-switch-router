import type { Metadata } from "next";
import { Noto_Sans_SC } from "next/font/google";
import { AppProviders } from "@/components/providers/app-providers";
import "./globals.css";

const notoSansSC = Noto_Sans_SC({
  weight: "variable",
  variable: "--font-noto-sans-sc-face",
  display: "swap",
  preload: false,
  adjustFontFallback: false,
});

export const metadata: Metadata = {
  title: "CC-Switch Router",
  description: "cc-switch-router dashboard and administration console",
  icons: {
    icon: "/router-logo.svg",
    shortcut: "/router-logo.svg",
    apple: "/router-logo.svg",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" className={notoSansSC.variable}>
      <body>
        <AppProviders>{children}</AppProviders>
      </body>
    </html>
  );
}
