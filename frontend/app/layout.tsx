import type { Metadata } from "next";
import { countryFlagFont } from "@/app/fonts";
import { AppProviders } from "@/components/providers/app-providers";
import "./globals.css";

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
    <html lang="en" className={countryFlagFont.variable}>
      <body>
        <AppProviders>{children}</AppProviders>
      </body>
    </html>
  );
}
