import type { Metadata } from "next";
import { Inter, JetBrains_Mono, Noto_Sans_SC, Source_Code_Pro } from "next/font/google";
import { AppProviders } from "@/components/providers/app-providers";
import "./globals.css";

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
  display: "swap",
});

/** CJK companion for Inter — clean geometric sans, readable at UI sizes. */
const notoSansSc = Noto_Sans_SC({
  subsets: ["latin"],
  weight: ["400", "500", "600", "700"],
  variable: "--font-noto-sans-sc",
  display: "swap",
});

const jetbrains = JetBrains_Mono({
  subsets: ["latin"],
  variable: "--font-jetbrains",
  display: "swap",
});

const sourceCodePro = Source_Code_Pro({
  subsets: ["latin"],
  variable: "--font-source-code-pro",
  display: "swap",
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
    <html lang="en">
      <body
        className={`${inter.variable} ${notoSansSc.variable} ${jetbrains.variable} ${sourceCodePro.variable}`}
      >
        <AppProviders>{children}</AppProviders>
      </body>
    </html>
  );
}
