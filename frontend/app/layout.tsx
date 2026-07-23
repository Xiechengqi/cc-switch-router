import type { Metadata } from "next";
import { Inter, JetBrains_Mono, Source_Code_Pro } from "next/font/google";
import "./globals.css";

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
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
      <body className={`${inter.variable} ${jetbrains.variable} ${sourceCodePro.variable}`}>
        {children}
      </body>
    </html>
  );
}
